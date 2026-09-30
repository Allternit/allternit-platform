import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import JSZip from "jszip";
import { hasErrors, type DirectoryFinding } from "@allternit/apps-server";
import { parseArgs, str } from "../args.js";
import { CliError, type Context } from "../context.js";
import { liveScan } from "../live.js";
import { printFindings } from "./test.js";

export const SUBMISSION_FILE = "allternit.submission.json";
export const DEFAULT_API = "https://api.allternit.com";

/** The same limits the directory form enforces (allternit-ai `directory/submission.ts`). */
export const SUBMISSION_LIMITS = { name: 30, shortDescription: 80, longDescription: 4000, positiveTests: 5, negativeTests: 3 } as const;

export interface TestCase {
  prompt: string;
  expected: string;
}

export interface AppSubmission {
  name: string;
  shortDescription: string;
  longDescription: string;
  developer: string;
  privacyUrl: string;
  termsUrl: string;
  icon: string;
  screenshots: string[];
  positiveTests: TestCase[];
  negativeTests: TestCase[];
  reviewerCredentials: string;
}

const err = (code: string, message: string, subject: string): DirectoryFinding => ({ severity: "error", code, message, subject });

function httpsPublic(value: unknown): boolean {
  if (typeof value !== "string") return false;
  try {
    const u = new URL(value);
    return u.protocol === "https:" && !/^(localhost|127\.|10\.|192\.168\.|0\.0\.0\.0|\[::1\])/.test(u.hostname);
  } catch {
    return false;
  }
}

export function validateSubmission(s: Partial<AppSubmission>): DirectoryFinding[] {
  const out: DirectoryFinding[] = [];
  const L = SUBMISSION_LIMITS;
  const text = (v: unknown) => (typeof v === "string" ? v : "");
  if (!text(s.name).trim()) out.push(err("form.name-required", "Name is required.", "name"));
  else if (text(s.name).trim().length > L.name) out.push(err("form.name-too-long", `Name must be ${L.name} characters or fewer.`, "name"));
  if (!text(s.shortDescription).trim()) out.push(err("form.short-required", "Short description is required.", "shortDescription"));
  else if (text(s.shortDescription).length > L.shortDescription) out.push(err("form.short-too-long", `Short description must be ${L.shortDescription} characters or fewer.`, "shortDescription"));
  if (!text(s.longDescription).trim()) out.push(err("form.long-required", "Long description is required.", "longDescription"));
  else if (text(s.longDescription).length > L.longDescription) out.push(err("form.long-too-long", `Long description must be ${L.longDescription} characters or fewer.`, "longDescription"));
  if (!text(s.developer).trim()) out.push(err("form.developer-required", "Developer name is required.", "developer"));
  if (!httpsPublic(s.privacyUrl)) out.push(err("form.privacy-url", "Privacy policy must be a public https:// URL.", "privacyUrl"));
  if (!httpsPublic(s.termsUrl)) out.push(err("form.terms-url", "Terms of service must be a public https:// URL.", "termsUrl"));
  if (!text(s.icon).trim()) out.push(err("form.icon-required", "Icon is required.", "icon"));
  if (!Array.isArray(s.screenshots) || s.screenshots.length === 0) out.push(err("form.screenshots-required", "At least one screenshot is required.", "screenshots"));
  const filled = (t: TestCase | undefined) => !!t && text(t.prompt).trim() && text(t.expected).trim();
  if ((s.positiveTests ?? []).filter(filled).length < L.positiveTests) out.push(err("form.positive-tests", `Provide ${L.positiveTests} positive test cases (prompt and expected result).`, "positiveTests"));
  if ((s.negativeTests ?? []).filter(filled).length < L.negativeTests) out.push(err("form.negative-tests", `Provide ${L.negativeTests} negative test cases (prompts that must not trigger the app).`, "negativeTests"));
  return out;
}

const toBase64 = (bytes: Uint8Array) => Buffer.from(bytes).toString("base64");

async function api<T>(ctx: Context, base: string, token: string, path: string, body: unknown): Promise<T> {
  const res = await ctx.fetch(`${base.replace(/\/$/, "")}${path}`, {
    method: "POST",
    headers: { "content-type": "application/json", authorization: `Bearer ${token}` },
    body: JSON.stringify(body),
  });
  const raw = await res.text();
  if (!res.ok) throw new CliError(`${path} failed (${res.status}): ${raw.slice(0, 500)}`);
  return (raw ? JSON.parse(raw) : {}) as T;
}

function tokenOf(ctx: Context, flags: Record<string, string | boolean>): string {
  const token = str(flags.token) ?? ctx.env.ALLTERNIT_TOKEN;
  if (!token) throw new CliError("A token is required: pass --token or set ALLTERNIT_TOKEN.");
  return token;
}

/** `allternit domain <host> [--check]`: request the domain-verification token, or ask the server to check it. */
export async function domain(ctx: Context, argv: string[]): Promise<void> {
  const { positional, flags } = parseArgs(argv, ["check"]);
  const host = positional[0];
  if (!host) throw new CliError("Usage: allternit domain <host> [--check]");
  const base = str(flags.api) ?? ctx.env.ALLTERNIT_API_URL ?? DEFAULT_API;
  const token = tokenOf(ctx, flags);
  if (flags.check) {
    ctx.log(JSON.stringify(await api(ctx, base, token, "/api/v1/developers/domain-tokens/check", { host }), null, 2));
    return;
  }
  const { token: challenge } = await api<{ token: string }>(ctx, base, token, "/api/v1/developers/domain-tokens", { host });
  ctx.log(`Serve this exact value at https://${host}/.well-known/allternit-apps-challenge, then run: allternit domain ${host} --check`);
  ctx.log(challenge);
}

/**
 * `allternit submit [app.zip] [--submission allternit.submission.json] [--token T] [--api URL] [--dry-run]`
 * Validates the listing form and the package, scans the hosted server the package points at, then
 * posts the submission for directory review. The token is sent only as a bearer header, never printed.
 * Live API calls need a real token and a hosted server; everything before the POST is unit-tested.
 */
export async function submit(ctx: Context, argv: string[]): Promise<void> {
  const { positional, flags } = parseArgs(argv, ["dry-run", "no-scan"]);
  const zipPath = resolve(ctx.cwd, positional[0] ?? (existsSync(resolve(ctx.cwd, "package.zip")) ? "package.zip" : ""));
  if (!positional[0] && !existsSync(zipPath)) throw new CliError("Usage: allternit submit <package.zip> (build it with `allternit package`).");
  if (!existsSync(zipPath)) throw new CliError(`Package not found: ${zipPath}`);
  const bytes = readFileSync(zipPath);
  const zip = await JSZip.loadAsync(bytes).catch(() => null);
  if (!zip) throw new CliError(`${zipPath} is not a readable ZIP archive.`);
  const pluginJson = zip.file("plugin.json");
  const mcpJson = zip.file("mcp.json");
  if (!pluginJson || !mcpJson) throw new CliError("Package must contain plugin.json and mcp.json (build it with `allternit package`).");
  const plugin = JSON.parse(await pluginJson.async("string")) as { name?: string; description?: string; author?: string | { name?: string } };
  const servers = Object.values((JSON.parse(await mcpJson.async("string")) as { mcpServers?: Record<string, { url?: string }> }).mcpServers ?? {});
  const serverUrl = servers[0]?.url;
  if (servers.length !== 1 || !serverUrl) throw new CliError("mcp.json must declare exactly one server with a url.");

  const submissionFile = resolve(ctx.cwd, str(flags.submission) ?? SUBMISSION_FILE);
  if (!existsSync(submissionFile)) throw new CliError(`${SUBMISSION_FILE} not found. Create it with the listing fields (developer, privacyUrl, termsUrl, icon, screenshots, longDescription, positiveTests, negativeTests, reviewerCredentials).`);
  const fromFile = JSON.parse(readFileSync(submissionFile, "utf8")) as Partial<AppSubmission>;
  const author = typeof plugin.author === "string" ? plugin.author : plugin.author?.name;
  const submission: AppSubmission = {
    name: fromFile.name ?? plugin.name ?? "",
    shortDescription: fromFile.shortDescription ?? (plugin.description ?? "").slice(0, SUBMISSION_LIMITS.shortDescription),
    longDescription: fromFile.longDescription ?? "",
    developer: fromFile.developer ?? author ?? "",
    privacyUrl: fromFile.privacyUrl ?? "",
    termsUrl: fromFile.termsUrl ?? "",
    icon: fromFile.icon ?? "",
    screenshots: fromFile.screenshots ?? [],
    positiveTests: fromFile.positiveTests ?? [],
    negativeTests: fromFile.negativeTests ?? [],
    reviewerCredentials: fromFile.reviewerCredentials ?? "",
  };

  const formFindings = validateSubmission(submission);
  if (hasErrors(formFindings)) {
    printFindings(ctx, formFindings);
    throw new CliError("Fix the listing fields above.");
  }

  let snapshot: unknown = null;
  if (!flags["no-scan"]) {
    ctx.log(`Scanning ${serverUrl}`);
    let scan;
    try {
      scan = await liveScan(serverUrl);
    } catch (e) {
      throw new CliError(`Could not reach ${serverUrl}: ${e instanceof Error ? e.message : String(e)}. The server must be hosted before you submit.`);
    }
    if (scan.findings.length) printFindings(ctx, scan.findings);
    if (hasErrors(scan.findings)) throw new CliError("Fix the scan errors above.");
    snapshot = scan.input;
  }

  const appId = str(flags["app-id"]);
  const payload = { ...submission, packageZip: toBase64(bytes), ...(snapshot ? { snapshot } : {}), ...(appId ? { appId } : {}) };
  if (flags["dry-run"]) {
    ctx.log(JSON.stringify({ ...payload, packageZip: `<${bytes.length} bytes base64>`, reviewerCredentials: submission.reviewerCredentials ? "<redacted>" : "" }, null, 2));
    ctx.log("Dry run: nothing was sent.");
    return;
  }
  const base = str(flags.api) ?? ctx.env.ALLTERNIT_API_URL ?? DEFAULT_API;
  const record = await api<{ id: string; state: string }>(ctx, base, tokenOf(ctx, flags), "/api/v1/miniapps/submissions", payload);
  ctx.log(`Submitted ${submission.name}: ${record.id} (${record.state}). Using your app does not wait for review; the directory listing does.`);
}
