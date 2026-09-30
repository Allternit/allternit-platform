// Deterministic hard rules for the PreToolUse guard. These run FIRST and are
// final: nothing the model says can relax them. Pure functions, table-tested.
import { homedir } from "node:os";
import { isAbsolute, normalize, resolve } from "node:path";

export type Verdict = "deny" | "ask";

export interface HardRuleResult {
  verdict: Verdict;
  rule: string;
  reason: string;
}

export interface ToolCall {
  tool_name: string;
  tool_input: Record<string, unknown>;
  cwd?: string;
}

const HOME = homedir();

// ---------------------------------------------------------------- shell parsing

/** Split a command line into simple-command segments on ; && || | & and newlines (quote-aware). */
export function splitSegments(cmd: string): string[] {
  const out: string[] = [];
  let cur = "";
  let q: '"' | "'" | null = null;
  for (let i = 0; i < cmd.length; i++) {
    const c = cmd[i];
    if (q) {
      if (c === q) q = null;
      else if (c === "\\" && q === '"' && i + 1 < cmd.length) { cur += c + cmd[++i]; continue; }
      cur += c;
      continue;
    }
    if (c === '"' || c === "'") { q = c; cur += c; continue; }
    if (c === "\\" && i + 1 < cmd.length) { cur += c + cmd[++i]; continue; }
    if (c === ";" || c === "\n" || c === "|" || c === "&" || c === "(" || c === ")" || c === "`") {
      if (cur.trim()) out.push(cur.trim());
      cur = "";
      continue;
    }
    if (c === "$" && cmd[i + 1] === "(") { if (cur.trim()) out.push(cur.trim()); cur = ""; i++; continue; }
    cur += c;
  }
  if (cur.trim()) out.push(cur.trim());
  return out;
}

/** Quote-aware word split (no globbing / expansion). */
export function words(seg: string): string[] {
  const out: string[] = [];
  let cur = "";
  let q: '"' | "'" | null = null;
  let has = false;
  for (let i = 0; i < seg.length; i++) {
    const c = seg[i];
    if (q) {
      if (c === q) q = null;
      else if (c === "\\" && q === '"' && i + 1 < seg.length) cur += seg[++i];
      else cur += c;
      continue;
    }
    if (c === '"' || c === "'") { q = c; has = true; continue; }
    if (c === "\\" && i + 1 < seg.length) { cur += seg[++i]; has = true; continue; }
    if (/\s/.test(c)) { if (has || cur) out.push(cur); cur = ""; has = false; continue; }
    cur += c;
    has = true;
  }
  if (has || cur) out.push(cur);
  return out;
}

const WRAPPERS = new Set(["sudo", "doas", "command", "builtin", "nohup", "time", "exec", "env", "nice", "xargs", "caffeinate", "npx", "bunx"]);
// `pnpm exec x`, `pnpm dlx x`, `yarn dlx x`, `bun x x`, `npm exec x` run x.
const RUNNER_SUBCMDS: Record<string, Set<string>> = {
  pnpm: new Set(["exec", "dlx"]), yarn: new Set(["dlx", "exec"]), bun: new Set(["x"]), npm: new Set(["exec"]),
};

/** Drop leading env assignments and wrapper commands; return [argv0-basename, ...args]. */
export function argv(seg: string): string[] {
  const w = words(seg);
  let i = 0;
  while (i < w.length) {
    if (/^[A-Za-z_][A-Za-z0-9_]*=/.test(w[i])) { i++; continue; }
    const base = w[i].split("/").pop()!;
    if (RUNNER_SUBCMDS[base]?.has(w[i + 1] ?? "")) {
      i += 2;
      while (i < w.length && w[i].startsWith("-")) i++;
      continue;
    }
    if (WRAPPERS.has(base)) {
      i++;
      while (i < w.length && w[i].startsWith("-")) i++; // wrapper flags (sudo -u x is rare; good enough)
      continue;
    }
    break;
  }
  if (i >= w.length) return [];
  return [w[i].split("/").pop()!, ...w.slice(i + 1)];
}

// ---------------------------------------------------------------- paths

export function expandHome(p: string): string {
  return p
    .replace(/^~(?=$|\/)/, HOME)
    .replace(/^\$\{HOME\}(?=$|\/)/, HOME)
    .replace(/^\$HOME(?=$|\/)/, HOME);
}

const TMP_ROOTS = ["/tmp", "/private/tmp", "/var/folders", "/private/var/folders", process.env.TMPDIR?.replace(/\/$/, "")]
  .filter(Boolean) as string[];

export function isTmpPath(abs: string): boolean {
  return TMP_ROOTS.some((r) => abs === r || abs.startsWith(`${r}/`)) && !/(^|\/)\.\.(\/|$)/.test(abs);
}

function toAbs(p: string, cwd?: string): string | null {
  const e = expandHome(p);
  if (/[$`*?[{]/.test(e.replace(/\*$/, ""))) return null; // unresolved variable / glob mid-path
  if (isAbsolute(e)) return normalize(e);
  if (!cwd) return null;
  return resolve(cwd, e);
}

/** Paths whose recursive force-removal is catastrophic regardless of intent. */
function isCatastrophic(raw: string, abs: string | null): boolean {
  if (/^(\/|\/\*|~|~\/|~\/\*|\$HOME\/?\*?|\$\{HOME\}\/?\*?|\.|\.\/|\.\.|\.\.\/|\*|\.\*)$/.test(raw)) return true;
  if (!abs) return false;
  const a = abs.replace(/\/\*$/, "").replace(/\/$/, "") || "/";
  if (a === "/" || a === HOME) return true;
  const depth = a.split("/").filter(Boolean).length;
  if (depth <= 2) return true; // /usr, /Users/joe, /opt/homebrew …
  const homeChild = a.startsWith(`${HOME}/`) ? a.slice(HOME.length + 1) : null;
  if (homeChild && /^(Desktop|Documents|Downloads|Library|Pictures|Movies|Music|\.ssh|\.aws|\.config|\.claude|\.allternit|\.gnupg)$/.test(homeChild)) return true;
  return false;
}

// ---------------------------------------------------------------- credential paths

const ENV_OK = /\.env\.(example|sample|template|dist|defaults)$/;
export function isCredentialPath(p: string): "ssh" | "aws" | "env" | null {
  const e = expandHome(p);
  if (/(^|\/)\.ssh(\/|$)/.test(e)) return "ssh";
  if (/(^|\/)\.aws(\/|$)/.test(e)) return "aws";
  const base = e.split("/").pop() ?? "";
  if (/^\.env(\..+)?$/.test(base) && !ENV_OK.test(base)) return "env";
  return null;
}

// ---------------------------------------------------------------- rules

function rmRule(a: string[], cwd?: string): HardRuleResult | null {
  if (a[0] !== "rm") return null;
  let recursive = false, force = false;
  const targets: string[] = [];
  let endOpts = false;
  for (const x of a.slice(1)) {
    if (!endOpts && x === "--") { endOpts = true; continue; }
    if (!endOpts && x.startsWith("--")) {
      if (x === "--recursive") recursive = true;
      if (x === "--force") force = true;
      continue;
    }
    if (!endOpts && x.startsWith("-") && x.length > 1) {
      if (/[rR]/.test(x)) recursive = true;
      if (/f/.test(x)) force = true;
      continue;
    }
    targets.push(x);
  }
  if (!recursive || !force) return null;
  if (targets.length === 0) return null;
  let worst: HardRuleResult | null = null;
  for (const t of targets) {
    const abs = toAbs(t, cwd);
    const tmpRoot = abs !== null && TMP_ROOTS.includes(abs.replace(/\/\*?$/, ""));
    if (abs && isTmpPath(abs) && !tmpRoot) continue;
    if (tmpRoot || isCatastrophic(t, abs)) {
      return { verdict: "deny", rule: "rm-rf-catastrophic", reason: `rm -rf on "${t}" (root, home, or a top-level directory)` };
    }
    if (isCredentialPath(t)) {
      return { verdict: "deny", rule: "rm-rf-credentials", reason: `rm -rf on credential path "${t}"` };
    }
    worst = { verdict: "ask", rule: "rm-rf-outside-tmp", reason: `rm -rf outside a temp dir: "${t}"` };
  }
  return worst;
}

const PROTECTED = /^(refs\/heads\/)?(main|master)$/;
function gitPushRule(a: string[]): HardRuleResult | null {
  if (a[0] !== "git") return null;
  // skip global opts like -C <dir>, -c k=v
  let i = 1;
  while (i < a.length && a[i].startsWith("-")) { if (a[i] === "-C" || a[i] === "-c") i++; i++; }
  if (a[i] !== "push") return null;
  const rest = a.slice(i + 1);
  const force = rest.some((x) => x === "--force" || x === "-f" || x.startsWith("--force-with-lease") || x === "--force-if-includes" || (/^-[a-zA-Z]+$/.test(x) && x.includes("f")));
  const del = rest.includes("--delete") || rest.includes("-d");
  const positional = rest.filter((x) => !x.startsWith("-"));
  const refspecs = positional.slice(1); // first positional is the remote
  const plusRef = refspecs.some((r) => r.startsWith("+"));
  const dst = (r: string) => {
    const s = r.replace(/^\+/, "");
    return s.includes(":") ? s.split(":").pop()! : s;
  };
  const touchesProtected = refspecs.some((r) => PROTECTED.test(dst(r)) || (r.startsWith(":") && PROTECTED.test(r.slice(1))));
  if ((del || refspecs.some((r) => r.startsWith(":"))) && touchesProtected) {
    return { verdict: "deny", rule: "git-delete-main", reason: "deleting main/master on a remote" };
  }
  if (!(force || plusRef)) return null;
  if (touchesProtected) return { verdict: "deny", rule: "git-force-push-main", reason: "force push to main/master" };
  if (rest.includes("--all") || rest.includes("--mirror")) {
    return { verdict: "deny", rule: "git-force-push-all", reason: "force push of all refs (includes main/master)" };
  }
  if (refspecs.length === 0 || refspecs.some((r) => /^(\+)?HEAD$/.test(r))) {
    return { verdict: "ask", rule: "git-force-push-unknown-branch", reason: "force push without an explicit non-main refspec" };
  }
  return null;
}

const WRITE_CMDS = new Set(["rm", "mv", "cp", "tee", "chmod", "chown", "truncate", "shred", "dd", "ln", "sed"]);
function credentialBashRule(seg: string, a: string[]): HardRuleResult | null {
  const toks = words(seg);
  const hits = toks.map((t) => t.replace(/^[<>]+/, "").replace(/^[A-Za-z_][A-Za-z0-9_]*=/, "")).filter((t) => isCredentialPath(t));
  if (!hits.length) return null;
  const redirectWrite = hits.some((h) => new RegExp(`>{1,2}\\s*['"]?${h.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}`).test(seg));
  if (redirectWrite || WRITE_CMDS.has(a[0])) {
    const kind = isCredentialPath(hits[0]);
    if (kind === "ssh" || kind === "aws" || a[0] === "rm" || a[0] === "shred") {
      return { verdict: "deny", rule: "credential-write", reason: `writes/removes credential path "${hits[0]}"` };
    }
    return { verdict: "ask", rule: "credential-write", reason: `modifies env file "${hits[0]}"` };
  }
  return { verdict: "ask", rule: "credential-read", reason: `touches credential path "${hits[0]}"` };
}

const PROD = /\b(prod|production)\b|--remote\b|RAILS_ENV=production|NODE_ENV=production|--env[= ]prod/i;
const NONLOCAL_DB = /(DATABASE_URL|PGHOST|POSTGRES_URL|DB_HOST)=['"]?(?![^\s'"]*(localhost|127\.0\.0\.1|::1|\.local\b|sqlite|file:))[^\s'"]+/i;
function migrationRule(seg: string, a: string[]): HardRuleResult | null {
  const s = seg;
  const always: [RegExp, string][] = [
    [/\bprisma\s+migrate\s+deploy\b/, "prisma migrate deploy"],
    [/\bsupabase\s+db\s+push\b/, "supabase db push"],
    [/\bwrangler\s+d1\s+migrations\s+apply\b.*--remote\b/, "wrangler d1 migrations apply --remote"],
    [/\bwrangler\s+d1\s+execute\b.*--remote\b.*(--file|--command)/, "wrangler d1 execute --remote"],
  ];
  for (const [re, what] of always) if (re.test(s)) return { verdict: "ask", rule: "prod-migration", reason: `${what} targets a production database` };
  const migrates = /\bmigrat(e|ion|ions)\b|db:migrate|\bdb\s+push\b|\balembic\s+upgrade\b|\bflyway\b|\bdbmate\s+up\b|\bgoose\b.*\bup\b/i.test(s)
    || (a[0] === "psql" && /(-f|--file)\s*\S*migrat/i.test(s));
  if (migrates && (PROD.test(s) || NONLOCAL_DB.test(s))) {
    return { verdict: "ask", rule: "prod-migration", reason: "database migration against a production/non-local target" };
  }
  return null;
}

// Deploy / publish from the shell: the CLAUDE.md review gate ("deploys preview
// before they run") must not depend on a model noticing them.
function deployPublishRule(seg: string, a: string[]): HardRuleResult | null {
  const checks: [boolean, string][] = [
    [a[0] === "wrangler" && /\b(pages\s+deploy|deploy|publish)\b/.test(seg) && !/--dry-run\b/.test(seg), "wrangler deploy"],
    [a[0] === "vercel" && /(^|\s)--prod\b/.test(seg), "vercel --prod"],
    [a[0] === "netlify" && /\bdeploy\b/.test(seg) && /--prod\b/.test(seg), "netlify deploy --prod"],
    [/^(npm|pnpm|yarn|bun|cargo)$/.test(a[0]) && a[1] === "publish" && !/--dry-run\b/.test(seg), `${a[0]} publish`],
    [a[0] === "gh" && a[1] === "release" && a[2] === "create", "gh release create"],
  ];
  const hit = checks.find(([c]) => c);
  return hit ? { verdict: "ask", rule: "deploy-publish", reason: `${hit[1]} publishes/deploys — preview and human go-ahead required` } : null;
}

const MONEY_DEPLOY_TOOL = /(^|__)(stripe_[a-z0-9_]+|cloudflare_deploy[a-z0-9_]*)$/i;
function mcpConfirmRule(call: ToolCall): HardRuleResult | null {
  if (!MONEY_DEPLOY_TOOL.test(call.tool_name)) return null;
  const c = call.tool_input?.confirm;
  if (c === true || c === "true" || c === 1) {
    return { verdict: "ask", rule: "mcp-confirm-true", reason: `${call.tool_name} with confirm:true is a live money/deploy action — human approval required` };
  }
  return null;
}

const FILE_TOOLS = new Set(["Read", "Write", "Edit", "MultiEdit", "NotebookEdit"]);
function fileToolRule(call: ToolCall): HardRuleResult | null {
  if (!FILE_TOOLS.has(call.tool_name)) return null;
  const p = String(call.tool_input.file_path ?? call.tool_input.notebook_path ?? "");
  const kind = p ? isCredentialPath(p) : null;
  if (!kind) return null;
  if (call.tool_name === "Read") return { verdict: "ask", rule: "credential-read", reason: `reads credential path "${p}"` };
  if (kind === "env") return { verdict: "ask", rule: "credential-write", reason: `edits env file "${p}"` };
  return { verdict: "deny", rule: "credential-write", reason: `writes credential path "${p}"` };
}

/** Evaluate all hard rules; deny beats ask. Returns null when no hard rule fires. */
export function evaluateHardRules(call: ToolCall): HardRuleResult | null {
  const results: HardRuleResult[] = [];
  const push = (r: HardRuleResult | null) => r && results.push(r);
  push(mcpConfirmRule(call));
  push(fileToolRule(call));
  if (call.tool_name === "Bash") {
    const cmd = String(call.tool_input.command ?? "");
    for (const seg of splitSegments(cmd)) {
      const a = argv(seg);
      if (!a.length) continue;
      push(rmRule(a, call.cwd));
      push(gitPushRule(a));
      push(credentialBashRule(seg, a));
      push(migrationRule(seg, a));
      push(deployPublishRule(seg, a));
      // bash -c / sh -c "<inner>": recurse into the inner command string.
      if ((a[0] === "bash" || a[0] === "sh" || a[0] === "zsh") && a[1] === "-c" && a[2]) {
        push(evaluateHardRules({ ...call, tool_input: { command: a[2] } }));
      }
    }
  }
  return results.find((r) => r.verdict === "deny") ?? results[0] ?? null;
}
