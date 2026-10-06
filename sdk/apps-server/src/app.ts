import { createMcpHandler, McpServer, type McpHttpHandler } from "@modelcontextprotocol/server";
import { registerAppResource, registerAppTool, RESOURCE_MIME_TYPE } from "@modelcontextprotocol/ext-apps/server";

import { validateLookPack, type LookPack } from "@allternit/look-packs";
import {
  hasErrors,
  isWellFormedCspDomain,
  scanInput,
  type DirectoryFinding,
  type McpAppToolDefinition,
  type ScanResource,
} from "./lint.js";
import { MANIFEST_SCHEMA, type AppManifest, type ManifestPermissions } from "./manifest.js";
import { z } from "zod";
import { checkEventDefinitions, registerEvents, type AppEventsInput } from "./events.js";
import type { AppToolDef } from "./tool.js";
import type { ViewDef } from "./view.js";

export interface AppPermissionsInput {
  auth?: "none" | "oauth";
  /** Extra hosts beyond the Views' CSP, e.g. APIs the server calls. */
  network?: string[];
  notes?: string;
}

export interface DefineAppInput {
  name: string;
  version?: string;
  description?: string;
  author?: string;
  icon?: string;
  homepage?: string;
  tools: AppToolDef[];
  views?: ViewDef[];
  lookPack?: LookPack;
  permissions?: AppPermissionsInput;
  /** Server instructions shown to the model. */
  instructions?: string;
  /** Path the server answers on. Default /mcp. */
  path?: string;
  /**
   * Optional MCP Events (`events/list|subscribe|unsubscribe`, webhook delivery
   * signed with Standard Webhooks). See `@allternit/mcp-events` for envelopes
   * and `deliverWebhook`.
   */
  events?: AppEventsInput;
}

export interface AllternitApp {
  readonly input: DefineAppInput;
  manifest(options?: { url?: string }): AppManifest;
  /** Static checks: the directory scan rules over the declared tools and views. */
  lint(): DirectoryFinding[];
  /**
   * A fresh MCP server with every tool and View registered. It serves both
   * protocol eras: 2026-07-28 (`server/discover`, per-request `_meta`) and the
   * legacy `initialize` handshake. Tools list in name order.
   */
  createServer(): McpServer;
  /**
   * The MCP endpoint as a web-standard `fetch` handler (Workers, Bun, Deno, or
   * Node via `listen`): 2026-07-28 requests are served statelessly by a fresh
   * server per request, legacy `initialize` clients get the stateless 2025-era
   * fallback from the same factory. Built here, next to `createServer`, so the
   * handler and the servers always come from one copy of the SDK (a loader such
   * as tsx can otherwise hand `listen` a second copy, and the SDK's
   * `instanceof` checks fail across copies).
   */
  createHandler(options?: { onerror?: (error: Error) => void }): McpHttpHandler;
}

const byKey = <T>(xs: T[], key: (x: T) => string) => [...xs].sort((a, b) => (key(a) < key(b) ? -1 : key(a) > key(b) ? 1 : 0));

const slug = (s: string) => s.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "") || "app";

export function defineApp(input: DefineAppInput): AllternitApp {
  if (!input.name?.trim()) throw new Error("defineApp: name is required");
  const views = input.views ?? [];
  const seenTools = new Set<string>();
  for (const t of input.tools) {
    if (seenTools.has(t.name)) throw new Error(`defineApp: duplicate tool ${t.name}`);
    seenTools.add(t.name);
  }
  const uris = new Set<string>();
  for (const v of views) {
    if (uris.has(v.uri)) throw new Error(`defineApp: duplicate view ${v.uri}`);
    uris.add(v.uri);
    for (const d of [...v.csp.connectDomains, ...v.csp.resourceDomains, ...(v.csp.frameDomains ?? []), ...(v.csp.baseUriDomains ?? [])]) {
      if (!isWellFormedCspDomain(d)) throw new Error(`defineApp: view ${v.uri} has a malformed or over-broad csp domain: ${JSON.stringify(d)}`);
    }
  }
  for (const t of input.tools) {
    if (t.viewUri && !uris.has(t.viewUri)) throw new Error(`defineApp: tool ${t.name} renders into ${t.viewUri}, which is not in views`);
  }
  if (input.events) checkEventDefinitions(input.events.definitions);
  if (input.lookPack) {
    const errors = validateLookPack(input.lookPack).filter((x) => x.severity === "error");
    if (errors.length) throw new Error(`defineApp: look pack invalid: ${errors.map((e) => e.message).join("; ")}`);
  }
  for (const v of views) {
    const submit = v.kit?.submitTool;
    if (submit && !seenTools.has(submit)) throw new Error(`defineApp: view ${v.uri} submits to ${submit}, which is not a tool`);
  }

  const version = input.version ?? "0.1.0";
  const path = input.path ?? "/mcp";

  const lookPack = input.lookPack ?? null;
  const renderCtx = { lookPack, appName: input.name, appVersion: version };

  function manifest(options: { url?: string } = {}): AppManifest {
    const network = new Set<string>(input.permissions?.network ?? []);
    for (const v of views) [...v.csp.connectDomains, ...v.csp.resourceDomains].forEach((d) => network.add(d));
    const permissions: ManifestPermissions = {
      auth: input.permissions?.auth ?? "none",
      network: [...network].sort(),
      writes: input.tools.some((t) => !t.annotations.readOnlyHint),
      destructive: input.tools.some((t) => t.annotations.destructiveHint),
      ...(input.permissions?.notes ? { notes: input.permissions.notes } : {}),
    };
    return {
      schema: MANIFEST_SCHEMA,
      name: input.name,
      version,
      ...(input.description ? { description: input.description } : {}),
      ...(input.author ? { author: input.author } : {}),
      ...(input.icon ? { icon: input.icon } : {}),
      ...(input.homepage ? { homepage: input.homepage } : {}),
      server: { name: slug(input.name), transport: "streamable-http", path, ...(options.url ? { url: options.url } : {}) },
      tools: input.tools.map((t) => ({
        name: t.name,
        title: t.title,
        description: t.description,
        inputs: Object.keys(t.input ?? {}),
        annotations: {
          readOnlyHint: t.annotations.readOnlyHint,
          destructiveHint: t.annotations.destructiveHint,
          ...(t.annotations.openWorldHint !== undefined ? { openWorldHint: t.annotations.openWorldHint } : {}),
          ...(t.annotations.idempotentHint !== undefined ? { idempotentHint: t.annotations.idempotentHint } : {}),
        },
        ...(t.viewUri ? { view: t.viewUri } : {}),
      })),
      views: views.map((v) => ({
        uri: v.uri,
        name: v.name,
        ...(v.description ? { description: v.description } : {}),
        csp: v.csp,
        prefersBorder: v.prefersBorder,
        source: v.kit ? ("kit" as const) : ("html" as const),
        ...(v.kit ? { kit: { tag: v.kit.tag, ...(v.kit.path ? { path: v.kit.path } : {}) } } : {}),
      })),
      lookPack,
      permissions,
    };
  }

  function lint(): DirectoryFinding[] {
    const tools: McpAppToolDefinition[] = input.tools.map((t) => ({
      name: t.name,
      title: t.title,
      description: t.description,
      inputSchema: { type: "object" },
      annotations: { ...t.annotations },
      _meta: t.viewUri ? { ui: { resourceUri: t.viewUri } } : undefined,
    }));
    const resources: ScanResource[] = views.map((v) => ({
      uri: v.uri,
      name: v.name,
      mimeType: RESOURCE_MIME_TYPE,
      meta: { ui: { csp: v.csp } },
    }));
    const findings = scanInput({ tools, resources });
    if (input.lookPack) {
      for (const x of validateLookPack(input.lookPack)) {
        if (x.severity !== "error") findings.push({ severity: x.severity, code: `lookpack.${x.code}`, message: x.message, subject: x.subject });
      }
    }
    return findings;
  }

  function createServer(): McpServer {
    const server = new McpServer(
      { name: slug(input.name), version },
      input.instructions ? { instructions: input.instructions } : undefined,
    );
    for (const t of byKey(input.tools, (x) => x.name)) {
      const config: Record<string, unknown> = {
        title: t.title,
        description: t.description,
        annotations: { ...t.annotations },
        ...(t.input ? { inputSchema: z.object(t.input) } : {}),
      };
      const handler = ((args: unknown) => t.handler(args as never)) as never;
      if (t.viewUri) {
        registerAppTool(server, t.name, { ...config, _meta: { ui: { resourceUri: t.viewUri } } } as never, handler);
      } else {
        server.registerTool(t.name, config as never, handler);
      }
    }
    for (const v of byKey(views, (x) => x.uri)) {
      registerAppResource(server, v.name, v.uri, { mimeType: RESOURCE_MIME_TYPE, ...(v.description ? { description: v.description } : {}) }, async () => ({
        contents: [
          {
            uri: v.uri,
            mimeType: RESOURCE_MIME_TYPE,
            text: v.render(renderCtx),
            _meta: { ui: { csp: v.csp, prefersBorder: v.prefersBorder } },
          },
        ],
      }));
    }
    if (input.events) registerEvents(server, input.events);
    return server;
  }

  function createHandler(options: { onerror?: (error: Error) => void } = {}): McpHttpHandler {
    return createMcpHandler(() => createServer(), {
      onerror: options.onerror ?? ((error) => console.error(`[${input.name}] mcp:`, error)),
    });
  }

  return { input, manifest, lint, createServer, createHandler };
}

