/**
 * allternit.app.json — the one manifest an app emits. It carries what the plugin
 * package needs (plugin.json + mcp.json) plus the layers Studio saves: tools,
 * views, look pack, permissions.
 *
 * The plugin types below are extracted from allternit-ai
 * `src/lib/ai/mcp/directory/plugin-package.ts` (PluginManifest, PluginMcpServer,
 * PluginSkill, ParsedPlugin) and the validation rules mirror its parser, so a
 * package built from this manifest parses there unchanged. Not forked: allternit-ai
 * should import these types from here.
 */
import type { LookPack } from "@allternit/look-packs";
import { validateLookPack } from "@allternit/look-packs";
import type { DirectoryFinding } from "./lint.js";

// ── Plugin package types (extracted) ────────────────────────────────────────

export interface PluginManifest {
  name: string;
  version?: string;
  description?: string;
  author?: string | { name?: string; url?: string };
  icon?: string;
  homepage?: string;
  [key: string]: unknown;
}

export interface PluginMcpServer {
  name: string;
  url: string;
  headers?: Record<string, string>;
}

export interface PluginSkill {
  name: string;
  content: string;
}

export interface ParsedPlugin {
  manifest: PluginManifest;
  skills: PluginSkill[];
  mcpServer: PluginMcpServer | null;
  /** path (under assets/) -> bytes */
  assets: Record<string, Uint8Array>;
}

// ── App manifest ────────────────────────────────────────────────────────────

export const MANIFEST_SCHEMA = "allternit.app/1";
export const MANIFEST_FILE = "allternit.app.json";

export interface ManifestTool {
  name: string;
  title: string;
  description: string;
  /** Input parameter names (the JSON schema is served live by tools/list). */
  inputs: string[];
  annotations: { readOnlyHint: boolean; destructiveHint: boolean; openWorldHint?: boolean; idempotentHint?: boolean };
  /** `ui://` URI of the View this tool renders into, when it has one. */
  view?: string;
}

export interface ManifestView {
  uri: string;
  name: string;
  description?: string;
  csp: { connectDomains: string[]; resourceDomains: string[]; frameDomains?: string[]; baseUriDomains?: string[] };
  prefersBorder: boolean;
  /** "kit" when composed from the view kit, "html" when authored by hand. */
  source: "kit" | "html";
  kit?: { tag: string; path?: string };
}

export interface ManifestPermissions {
  auth: "none" | "oauth";
  /** Every host the app's Views or server reach, for the install permission explanation. */
  network: string[];
  /** True when any tool is not read-only. */
  writes: boolean;
  /** True when any tool is destructive. */
  destructive: boolean;
  notes?: string;
}

export interface AppManifest {
  schema: typeof MANIFEST_SCHEMA;
  name: string;
  version: string;
  description?: string;
  author?: string;
  icon?: string;
  homepage?: string;
  server: { name: string; transport: "streamable-http"; path: string; url?: string };
  tools: ManifestTool[];
  views: ManifestView[];
  lookPack: LookPack | null;
  permissions: ManifestPermissions;
}

const f = (severity: DirectoryFinding["severity"], code: string, message: string, subject?: string): DirectoryFinding => ({
  severity,
  code,
  message,
  subject,
});

const isObj = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);

export function validateManifest(input: unknown): DirectoryFinding[] {
  if (!isObj(input)) return [f("error", "manifest.shape", `${MANIFEST_FILE} must be a JSON object.`)];
  const out: DirectoryFinding[] = [];
  if (input.schema !== MANIFEST_SCHEMA) out.push(f("error", "manifest.schema", `schema must be "${MANIFEST_SCHEMA}".`));
  if (typeof input.name !== "string" || !input.name.trim()) out.push(f("error", "plugin.name-missing", 'needs a non-empty "name".'));
  else if (input.name.length > 30) out.push(f("error", "plugin.name-too-long", "Name must be 30 characters or fewer."));
  if (typeof input.version !== "string" || !input.version) out.push(f("warning", "plugin.version-missing", 'has no "version".'));
  if (typeof input.description !== "string" || !input.description.trim()) out.push(f("warning", "plugin.description-missing", 'has no "description".'));
  if (!isObj(input.server) || input.server.transport !== "streamable-http") {
    out.push(f("error", "mcp.transport", "server.transport must be streamable-http."));
  }
  const tools = Array.isArray(input.tools) ? input.tools : null;
  const views = Array.isArray(input.views) ? input.views : null;
  if (!tools) out.push(f("error", "manifest.tools", "tools must be an array."));
  if (!views) out.push(f("error", "manifest.views", "views must be an array."));
  const uris = new Set((views ?? []).map((v) => (isObj(v) ? v.uri : undefined)));
  for (const t of tools ?? []) {
    if (!isObj(t) || typeof t.name !== "string") {
      out.push(f("error", "tool.name-missing", "Tool has no name."));
      continue;
    }
    const a = t.annotations;
    if (!isObj(a) || typeof a.readOnlyHint !== "boolean" || typeof a.destructiveHint !== "boolean") {
      out.push(f("error", "tool.annotations-missing", "Tool must state readOnlyHint and destructiveHint.", `tool:${t.name}`));
    }
    if (typeof t.view === "string" && !uris.has(t.view)) {
      out.push(f("error", "resource.tool-target-missing", `Tool references ${t.view}, which is not a declared view.`, `tool:${t.name}`));
    }
  }
  if (input.lookPack !== null && input.lookPack !== undefined) {
    for (const x of validateLookPack(input.lookPack)) {
      out.push(f(x.severity, `lookpack.${x.code}`, x.message, x.subject));
    }
  }
  if (!isObj(input.permissions)) out.push(f("error", "manifest.permissions", "permissions must be an object."));
  return out;
}

export interface PluginFiles {
  "plugin.json": PluginManifest;
  "mcp.json": { mcpServers: Record<string, { type: "streamable-http"; url: string }> };
}

/** The plugin.json + mcp.json halves of the package, in the layout `parsePluginZip` reads. */
export function toPluginFiles(manifest: AppManifest, url?: string): PluginFiles {
  const serverUrl = url ?? manifest.server.url;
  if (!serverUrl) throw new Error("A public https url is required to package the app.");
  if (!/^https:\/\//i.test(serverUrl)) throw new Error("The server url must be an https:// URL.");
  new URL(serverUrl);
  const pluginJson: PluginManifest = {
    name: manifest.name,
    version: manifest.version,
    ...(manifest.description ? { description: manifest.description } : {}),
    ...(manifest.author ? { author: manifest.author } : {}),
    ...(manifest.icon ? { icon: manifest.icon } : {}),
    ...(manifest.homepage ? { homepage: manifest.homepage } : {}),
  };
  return {
    "plugin.json": pluginJson,
    "mcp.json": { mcpServers: { [manifest.server.name]: { type: "streamable-http", url: serverUrl } } },
  };
}
