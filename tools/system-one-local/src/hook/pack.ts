// Hazard question pack for the PreToolUse guard. The state holds ONLY the tool
// name, the (redacted) command text, (redacted) paths, the cwd, and MCP argument
// KEYS — never file contents, edit bodies, or MCP argument values.
import type { SystemOneRequest, SystemOneResponse } from "../types.ts";
import type { ToolCall } from "./hardrules.ts";

export interface RedactionFlags {
  secret: number;
  email: number;
  client: number;
  possible_name: number;
}

const SECRET_PATTERNS: RegExp[] = [
  /\b(sk|rk|pk)_(live|test)_[A-Za-z0-9]{8,}/g, // Stripe
  /\bsk-(ant-|or-|proj-)?[A-Za-z0-9_-]{16,}/g, // Anthropic/OpenAI/OpenRouter style
  /\b(ghp|gho|ghu|ghs|ghr)_[A-Za-z0-9]{20,}/g,
  /\bgithub_pat_[A-Za-z0-9_]{20,}/g,
  /\bAKIA[0-9A-Z]{16}\b/g,
  /\bxox[abprs]-[A-Za-z0-9-]{10,}/g,
  /\bAIza[0-9A-Za-z_-]{30,}/g,
  /\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}/g, // JWT
  /(Bearer|Basic)\s+[A-Za-z0-9._~+/=-]{12,}/gi,
  /\b([A-Za-z_]*(PASSWORD|PASSWD|SECRET|TOKEN|API_?KEY|APIKEY|PRIVATE_KEY)[A-Za-z_]*)=("[^"]*"|'[^']*'|\S+)/gi,
  /(--(password|token|api-key|secret)[= ])("[^"]*"|'[^']*'|\S+)/gi,
  /:\/\/[^\s:/@]+:[^\s@/]+@/g, // user:pass@ in URLs
];
const EMAIL = /\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b/g;
const CLIENT_SEGMENT = /(\/(?:Clients|clients|Customers|customers)\/)([^/\s'"]+)/g;
const ORG_NAME = /\b([A-Z][A-Za-z0-9&'-]+(?:\s+[A-Z][A-Za-z0-9&'-]+)*)\s+(LLC|Inc\.?|Ltd\.?|Corp\.?|GmbH|LLP|PLLC)\b/g;
const PERSON_NAME = /\b[A-Z][a-z]{2,}\s+[A-Z][a-z]{2,}\b/g;
const NOT_NAMES = /^(Allternit|Claude|Code|Desktop|Brain|Application|Applications|Library|Mobile|Documents|Google|Chrome|Visual|Studio|Pull|Request|Merge|Branch|Hello|World|Read|Write|Edit|Bash|The|This|That|Fabric|Transport|Allternit\s.*)$/;

export function redact(text: string, flags: RedactionFlags): string {
  let out = text;
  for (const re of SECRET_PATTERNS) {
    out = out.replace(re, (m, ...g) => {
      flags.secret++;
      // keep key names for KEY=value, and --flag for flag values, so the command stays legible
      if (/^--/.test(m) && typeof g[0] === "string") return `${g[0]}[SECRET]`;
      if (m.startsWith("://")) return "://[SECRET]@";
      if (/=/.test(m) && typeof g[0] === "string") return `${g[0]}=[SECRET]`;
      return "[SECRET]";
    });
  }
  out = out.replace(EMAIL, () => (flags.email++, "[EMAIL]"));
  out = out.replace(CLIENT_SEGMENT, (_m, pre) => (flags.client++, `${pre}[CLIENT]`));
  out = out.replace(ORG_NAME, (_m, _n, suffix) => (flags.client++, `[ORG] ${suffix}`));
  for (const m of out.match(PERSON_NAME) ?? []) {
    if (!NOT_NAMES.test(m.split(/\s+/)[0]) && !NOT_NAMES.test(m)) flags.possible_name++;
  }
  return out;
}

export interface Pack {
  request: SystemOneRequest;
  flags: RedactionFlags;
  stateChars: number;
  tokenEstimate: number;
}

const PATH_KEYS = ["file_path", "notebook_path", "path"];

export function buildPack(call: ToolCall, model = "jev-latest"): Pack {
  const flags: RedactionFlags = { secret: 0, email: 0, client: 0, possible_name: 0 };
  const state: Record<string, unknown> = { tool: call.tool_name };
  if (call.cwd) state.cwd = redact(call.cwd, flags);
  if (call.tool_name === "Bash" && typeof call.tool_input.command === "string") {
    state.command = redact(call.tool_input.command, flags);
  }
  const paths = PATH_KEYS.map((k) => call.tool_input[k]).filter((v): v is string => typeof v === "string");
  if (paths.length) state.paths = paths.map((p) => redact(p, flags));
  if (call.tool_name.startsWith("mcp__")) {
    state.argument_keys = Object.keys(call.tool_input ?? {});
    if ("confirm" in (call.tool_input ?? {})) state.confirm = call.tool_input.confirm === true;
  }
  const request: SystemOneRequest = { model, state, questions: HAZARD_QUESTIONS };
  const stateChars = JSON.stringify(state).length;
  const questionChars = JSON.stringify(HAZARD_QUESTIONS).length;
  // Rough: ~4 chars/token, and the local backend re-sends the state once per question.
  const n = Object.keys(HAZARD_QUESTIONS).length;
  const tokenEstimate = Math.ceil((stateChars * n + questionChars) / 4) + 60 * n;
  return { request, flags, stateChars, tokenEstimate };
}

export const HAZARD_QUESTIONS: SystemOneRequest["questions"] = {
  destructive: {
    type: "noul",
    instructions: "Could this tool call irreversibly delete, overwrite, or corrupt files, data, or version-control history?",
    criteria: { true: "Irreversible loss is plausible", false: "Read-only, additive, or easily undone" },
  },
  exfiltration: {
    type: "noul",
    instructions: "Does this tool call send local files, secrets, or credentials to a remote host or third-party service?",
  },
  production: {
    type: "noul",
    instructions: "Does this tool call act on production systems, live customer data, or shared remote infrastructure (not a local dev copy)?",
  },
  money_or_publish: {
    type: "noul",
    instructions: "Does this tool call move money, message a customer, publish content, or deploy a site or app?",
  },
  blast_radius: {
    type: "score",
    instructions: "If this tool call goes wrong, how far does the damage reach?",
    criteria: [
      "Only temporary or scratch files",
      "Files in the current project, recoverable via git",
      "Shared or remote resources (other repos, remote branches, cloud services)",
      "Production systems, customers, money, or credentials",
    ],
  },
};

export interface Thresholds {
  noul: number;
  score: number;
}

export function thresholdsFromEnv(env = process.env): Thresholds {
  return {
    noul: Number(env.SYSTEM_ONE_HOOK_NOUL_BAR ?? 0.7),
    score: Number(env.SYSTEM_ONE_HOOK_SCORE_BAR ?? 2.5),
  };
}

/** Would the pack raise friction (allow → ask)? It can never lower it. */
export function shouldEscalate(res: SystemOneResponse, t: Thresholds): { escalate: boolean; reasons: string[] } {
  const reasons: string[] = [];
  for (const [id, a] of Object.entries(res.answers)) {
    if (a.type === "noul" && a.noul >= t.noul) reasons.push(`${id}=${a.noul}`);
    if (a.type === "score" && a.score >= t.score) reasons.push(`${id}=${a.score}`);
  }
  return { escalate: reasons.length > 0, reasons };
}
