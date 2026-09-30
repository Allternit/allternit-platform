/**
 * Directory scan rules (tool annotations, steering descriptions, ui:// resources and CSP).
 * Extracted verbatim from allternit-ai `src/lib/ai/mcp/directory/scanner.ts` so `allternit test`
 * enforces exactly what directory review does. allternit-ai should import these instead of keeping
 * its own copy once it depends on this package.
 */
export const MCP_APP_RESOURCE_MIME_TYPE = "text/html;profile=mcp-app";

export type FindingSeverity = "error" | "warning" | "info";

export interface DirectoryFinding {
  severity: FindingSeverity;
  code: string;
  message: string;
  subject?: string;
}

export const hasErrors = (findings: DirectoryFinding[]): boolean => findings.some((x) => x.severity === "error");

export interface McpAppResourceCsp {
  connectDomains?: string[];
  resourceDomains?: string[];
  frameDomains?: string[];
  baseUriDomains?: string[];
}

export interface McpAppToolDefinition {
  name: string;
  title?: string;
  description?: string;
  inputSchema?: unknown;
  annotations?: Record<string, unknown>;
  _meta?: Record<string, unknown>;
}

/** `_meta.ui.resourceUri`, falling back to the older flat `_meta["ui/resourceUri"]`. */
export function getMcpAppResourceUri(tool: McpAppToolDefinition): string | undefined {
  const ui = tool._meta?.ui as { resourceUri?: unknown } | undefined;
  const uri = ui?.resourceUri ?? tool._meta?.["ui/resourceUri"];
  return typeof uri === "string" ? uri : undefined;
}

export interface ScanResource {
  uri: string;
  name?: string;
  mimeType?: string;
  /** `_meta` from resources/read content or the list entry. */
  meta?: Record<string, unknown>;
}

export interface ScanInput {
  tools: McpAppToolDefinition[];
  resources: ScanResource[];
}

const f = (
  severity: DirectoryFinding["severity"],
  code: string,
  message: string,
  subject?: string,
): DirectoryFinding => ({ severity, code, message, subject });

/** Names that imply a mutation. */
const WRITE_NAME =
  /(^|[_\-.\s])(create|update|delete|remove|write|send|post|publish|set|add|edit|modify|drop|insert|upload|submit|cancel|pay|transfer|execute|run|deploy|approve|reject|revoke|reset|archive)([_\-.\s]|$)|^(create|update|delete|remove|write|send|post|publish|set|add|edit|modify|drop|insert|upload|submit|cancel|pay|transfer|execute|run|deploy|approve|reject|revoke|reset|archive)[A-Z]/i;
const READ_NAME = /^(get|list|search|find|read|fetch|show|view|lookup|query|describe|check)([_\-.\s]|[A-Z]|$)/i;

/** Descriptions that steer the model toward/away from other apps or override behaviour. */
const STEERING: Array<[RegExp, string]> = [
  [/\b(always|only|must|should)\s+(use|call|prefer)\s+this\b/i, "pushes the model to always use this tool"],
  [/\b(instead of|rather than|in place of)\s+(the\s+)?(other|another|any|built-?in|default)\b/i, "steers the model away from other tools"],
  [/\b(do not|don't|never)\s+(use|call)\s+(any\s+)?(other|another|different)\b/i, "tells the model not to use other tools"],
  [/\b(ignore|disregard|override)\s+(all\s+|any\s+)?(previous|prior|other|system)\b/i, "attempts to override prior instructions"],
  [/\b(before|prior to)\s+(using|calling)\s+(any\s+)?(other|another)\b/i, "forces ordering ahead of other tools"],
  [/\b(preferred|best|recommended|superior)\s+(tool|option|way|app)\b/i, "self-promotes over alternatives"],
  [/<\s*(important|system|instructions?)\s*>/i, "contains injected instruction markup"],
];

export function lintTools(tools: McpAppToolDefinition[]): DirectoryFinding[] {
  const out: DirectoryFinding[] = [];
  const seen = new Set<string>();
  for (const tool of tools) {
    const subject = `tool:${tool?.name ?? "(unnamed)"}`;
    if (!tool?.name || typeof tool.name !== "string") {
      out.push(f("error", "tool.name-missing", "Tool has no name.", subject));
      continue;
    }
    if (seen.has(tool.name)) out.push(f("error", "tool.name-duplicate", "Duplicate tool name.", subject));
    seen.add(tool.name);
    if (!/^[A-Za-z0-9_.\-]{1,64}$/.test(tool.name)) {
      out.push(f("warning", "tool.name-format", "Tool name should be 1-64 chars of letters, digits, _ . -", subject));
    }
    if (!tool.title?.trim() && !(tool.annotations as { title?: string } | undefined)?.title) {
      out.push(f("warning", "tool.title-missing", "Tool has no title.", subject));
    }
    const desc = tool.description?.trim();
    if (!desc) out.push(f("error", "tool.description-missing", "Tool has no description.", subject));
    else {
      if (desc.length < 15) out.push(f("warning", "tool.description-short", "Description is too short to guide the model.", subject));
      if (desc.length > 1000) out.push(f("warning", "tool.description-long", "Description exceeds 1000 characters.", subject));
      for (const [re, why] of STEERING) {
        if (re.test(desc)) out.push(f("warning", "tool.description-steering", `Description ${why}.`, subject));
      }
    }
    const schema = tool.inputSchema as { type?: unknown } | undefined;
    if (!schema || typeof schema !== "object") {
      out.push(f("error", "tool.input-schema-missing", "Tool has no inputSchema.", subject));
    } else if (schema.type !== "object") {
      out.push(f("error", "tool.input-schema-type", 'inputSchema.type must be "object".', subject));
    }

    const ann = tool.annotations as Record<string, unknown> | undefined;
    if (!ann || typeof ann !== "object") {
      out.push(f("warning", "tool.annotations-missing", "Tool has no annotations (readOnlyHint / destructiveHint / openWorldHint).", subject));
    } else {
      for (const key of ["readOnlyHint", "destructiveHint", "openWorldHint", "idempotentHint"]) {
        if (key in ann && typeof ann[key] !== "boolean") {
          out.push(f("error", "tool.annotation-type", `${key} must be a boolean.`, subject));
        }
      }
      if (ann.readOnlyHint === undefined) {
        out.push(f("warning", "tool.readonly-missing", "readOnlyHint is not declared.", subject));
      }
      if (ann.readOnlyHint === true && ann.destructiveHint === true) {
        out.push(f("error", "tool.annotations-contradict", "Tool is both readOnlyHint and destructiveHint.", subject));
      }
      if (ann.readOnlyHint === true && WRITE_NAME.test(tool.name)) {
        out.push(f("warning", "tool.readonly-implausible", "Name suggests a write but readOnlyHint is true.", subject));
      }
      if (ann.readOnlyHint === false && READ_NAME.test(tool.name) && !WRITE_NAME.test(tool.name) && ann.destructiveHint !== true) {
        out.push(f("info", "tool.readonly-implausible-inverse", "Name reads like a read but readOnlyHint is false.", subject));
      }
    }
    if (WRITE_NAME.test(tool.name) && ann?.readOnlyHint === undefined) {
      out.push(f("warning", "tool.write-without-hint", "Write-like tool name without readOnlyHint: false.", subject));
    }
  }
  return out;
}

const DOMAIN_KEYS: Array<keyof McpAppResourceCsp> = ["connectDomains", "resourceDomains", "frameDomains", "baseUriDomains"];
const HOST_RE = /^(https?:\/\/|wss?:\/\/)?(\*\.)?([a-z0-9-]+\.)+[a-z]{2,}(:\d{1,5})?$/i;

export function isWellFormedCspDomain(value: unknown): boolean {
  if (typeof value !== "string" || !value || /\s|;|'|"|,/.test(value)) return false;
  if (value === "*" || /^https?:\/\/\*$/.test(value)) return false;
  return HOST_RE.test(value) && !/[/?#]/.test(value.replace(/^[a-z]+:\/\//i, ""));
}

export function lintResources(resources: ScanResource[], tools: McpAppToolDefinition[] = []): DirectoryFinding[] {
  const out: DirectoryFinding[] = [];
  const uiResources = resources.filter((r) => r.uri?.startsWith("ui://"));
  const known = new Set(uiResources.map((r) => r.uri));
  for (const tool of tools) {
    const uri = getMcpAppResourceUri(tool);
    if (uri && !known.has(uri)) {
      out.push(f("error", "resource.tool-target-missing", `Tool references ${uri}, which resources/list does not expose.`, `tool:${tool.name}`));
    }
  }
  for (const r of uiResources) {
    const subject = `resource:${r.uri}`;
    if (r.mimeType && r.mimeType !== MCP_APP_RESOURCE_MIME_TYPE) {
      out.push(f("error", "resource.mime", `ui:// resource should use ${MCP_APP_RESOURCE_MIME_TYPE}.`, subject));
    }
    const ui = (r.meta?.ui ?? undefined) as { csp?: Record<string, unknown>; domain?: unknown } | undefined;
    const csp = ui?.csp;
    if (!csp) continue;
    let total = 0;
    for (const key of DOMAIN_KEYS) {
      const list = csp[key];
      if (list === undefined) continue;
      if (!Array.isArray(list)) {
        out.push(f("error", "csp.not-array", `csp.${key} must be an array of domains.`, subject));
        continue;
      }
      const dedup = new Set<string>();
      for (const d of list) {
        if (!isWellFormedCspDomain(d)) {
          out.push(f("error", "csp.malformed-domain", `csp.${key} has a malformed or over-broad entry: ${JSON.stringify(d)}.`, subject));
          continue;
        }
        if (dedup.has(d as string)) out.push(f("info", "csp.duplicate-domain", `csp.${key} lists ${d} more than once.`, subject));
        dedup.add(d as string);
        if (typeof d === "string" && (/^(https?|wss?):\/\/\*\./.test(d) || d.startsWith("*."))) {
          out.push(f("warning", "csp.wildcard-domain", `csp.${key} uses wildcard ${d}; list exact hosts.`, subject));
        }
        if (typeof d === "string" && /^(http|ws):\/\//i.test(d)) {
          out.push(f("warning", "csp.insecure-scheme", `csp.${key} allows non-TLS ${d}.`, subject));
        }
      }
      total += dedup.size;
    }
    if (total > 10) out.push(f("warning", "csp.not-minimal", `CSP declares ${total} domains; keep it minimal.`, subject));
    const frames = csp.frameDomains;
    if (Array.isArray(frames) && frames.length > 0) {
      out.push(f("warning", "csp.frame-domains", `frameDomains (${frames.join(", ")}) needs written justification; nested frames widen the attack surface.`, subject));
    }
  }
  return out;
}

export function scanInput(input: ScanInput): DirectoryFinding[] {
  const findings = [...lintTools(input.tools), ...lintResources(input.resources, input.tools)];
  if (input.tools.length === 0) findings.unshift(f("error", "server.no-tools", "Server exposes no tools."));
  if (!input.resources.some((r) => r.uri?.startsWith("ui://"))) {
    findings.push(f("warning", "server.no-ui-resources", "Server exposes no ui:// resources, so no MCP App View will render."));
  }
  return findings;
}

