import { afterEach, describe, expect, it } from "vitest";
import { Client, StreamableHTTPClientTransport } from "@modelcontextprotocol/client";
import {
  appTool,
  createHandler,
  defineApp,
  eventsError,
  EventsErrorCode,
  hasErrors,
  listen,
  toPluginFiles,
  validateManifest,
  view,
  z,
  type RunningApp,
} from "../src/index.js";

const TEMPS: Record<string, number> = { london: 14, tokyo: 22 };

function weather() {
  const card = view({
    uri: "ui://weather/card.html",
    name: "Weather card",
    kit: { tag: "allternit-detail", fields: { title: "city" } },
  });
  return defineApp({
    name: "Weather demo",
    version: "1.2.3",
    description: "Look up temperatures.",
    tools: [
      appTool({
        name: "get_temperature",
        title: "Get temperature",
        description: "Get the current temperature in Celsius for london or tokyo.",
        input: { city: z.string() },
        annotations: { readOnlyHint: true, destructiveHint: false },
        handler: async ({ city }) => ({ content: [{ type: "text", text: `${city}: ${TEMPS[city.toLowerCase()]}` }] }),
      }),
      appTool({
        name: "show_weather",
        title: "Show weather card",
        description: "Display a weather card for a city. Call only when the user wants to see it.",
        input: { city: z.string() },
        annotations: { readOnlyHint: true, destructiveHint: false },
        view: card,
        handler: async ({ city }) => ({
          content: [{ type: "text", text: `Showing ${city}` }],
          structuredContent: { city, temperatureC: TEMPS[city.toLowerCase()] },
        }),
      }),
    ],
    views: [card],
    lookPack: {
      id: "weather",
      name: "Weather",
      tokens: { color: { $type: "color", surface: { $value: "#ffffff" }, text: { $value: "#111111" }, brand: { $value: "#0b5fff" } } },
    },
  });
}

describe("appTool", () => {
  const base = {
    name: "t",
    title: "T",
    description: "Does a thing for the user.",
    handler: async () => ({ content: [] }),
  };

  it("requires readOnlyHint and destructiveHint to be stated", () => {
    expect(() => appTool({ ...base, annotations: { destructiveHint: false } as never })).toThrow(/readOnlyHint must be stated/);
    expect(() => appTool({ ...base, annotations: { readOnlyHint: true } as never })).toThrow(/destructiveHint must be stated/);
    expect(() => appTool({ ...base, annotations: undefined as never })).toThrow(/readOnlyHint/);
  });

  it("rejects a tool that is both read-only and destructive", () => {
    expect(() => appTool({ ...base, annotations: { readOnlyHint: true, destructiveHint: true } })).toThrow(/both/);
  });

  it("records the view uri from a view or a string", () => {
    const v = view({ uri: "ui://a/b.html", name: "B", html: "<p>x</p>" });
    expect(appTool({ ...base, annotations: { readOnlyHint: true, destructiveHint: false }, view: v }).viewUri).toBe("ui://a/b.html");
    expect(appTool({ ...base, annotations: { readOnlyHint: true, destructiveHint: false }, view: "ui://a/c.html" }).viewUri).toBe("ui://a/c.html");
  });
});

describe("view", () => {
  it("validates the uri and needs exactly one of html or kit", () => {
    expect(() => view({ uri: "https://x", name: "x", html: "<p/>" })).toThrow(/ui:\/\//);
    expect(() => view({ uri: "ui://a/b.html", name: "x" })).toThrow(/exactly one/);
    expect(() => view({ uri: "ui://a/b.html", name: "x", html: "<p/>", kit: { tag: "allternit-card" } })).toThrow(/exactly one/);
  });

  it("renders kit views with look-pack css, the kit and the mount script", () => {
    const v = view({ uri: "ui://a/b.html", name: "B", kit: { tag: "allternit-table", path: "rows", fields: { columns: ["a"] } } });
    const html = v.render({ lookPack: { id: "p", name: "P", tokens: { color: { $type: "color", brand: { $value: "#0b5fff" } } } }, appName: "A", appVersion: "1" });
    expect(html).toContain("--vp-color-brand: #0b5fff;");
    expect(html).toContain("<allternit-table id=\"view\">");
    expect(html).toContain("customElements.define");
    expect(html).toContain("ui/initialize");
  });

  it("cannot be broken out of by app data in the config", () => {
    const v = view({ uri: "ui://a/b.html", name: "B", kit: { tag: "allternit-card", fields: { title: "</script><script>alert(1)</script>" } } });
    const html = v.render({ lookPack: null, appName: "A", appVersion: "1" });
    expect(html).not.toContain("</script><script>alert(1)");
    expect(html).toContain("\\u003c/script>");
  });

  it("injects into hand-written html after <head>, and adds the openai shim on request", () => {
    const v = view({ uri: "ui://a/b.html", name: "B", html: "<!doctype html><html><head><title>x</title></head><body/></html>", openaiCompat: true });
    const html = v.render({ lookPack: null, appName: "A", appVersion: "1" });
    expect(html.indexOf("window.allternit")).toBeLessThan(html.indexOf("<title>"));
    expect(html).toContain("window.openai");
  });
});

describe("defineApp", () => {
  it("rejects duplicates, dangling views and malformed csp", () => {
    const t = (name: string, v?: string) =>
      appTool({ name, title: name, description: "Some useful tool description.", annotations: { readOnlyHint: true, destructiveHint: false }, view: v, handler: async () => ({ content: [] }) });
    expect(() => defineApp({ name: "A", tools: [t("x"), t("x")] })).toThrow(/duplicate tool/);
    expect(() => defineApp({ name: "A", tools: [t("x", "ui://a/missing.html")] })).toThrow(/not in views/);
    const bad = view({ uri: "ui://a/b.html", name: "B", html: "<p/>", csp: { connectDomains: ["*"] } });
    expect(() => defineApp({ name: "A", tools: [t("x")], views: [bad] })).toThrow(/over-broad/);
  });

  it("emits a manifest with derived permissions that validates", () => {
    const m = weather().manifest({ url: "https://weather.example.com/mcp" });
    expect(m.tools.map((x) => x.name)).toEqual(["get_temperature", "show_weather"]);
    expect(m.tools[1].view).toBe("ui://weather/card.html");
    expect(m.views[0]).toMatchObject({ source: "kit", kit: { tag: "allternit-detail" } });
    expect(m.permissions).toMatchObject({ auth: "none", writes: false, destructive: false, network: [] });
    expect(m.lookPack?.id).toBe("weather");
    expect(validateManifest(m).filter((x) => x.severity === "error")).toEqual([]);
  });

  it("flags writes and destructive tools in permissions", () => {
    const app = defineApp({
      name: "W",
      tools: [
        appTool({ name: "delete_item", title: "Delete", description: "Delete an item permanently.", annotations: { readOnlyHint: false, destructiveHint: true }, handler: async () => ({ content: [] }) }),
      ],
    });
    expect(app.manifest().permissions).toMatchObject({ writes: true, destructive: true });
  });

  it("validateManifest catches missing annotations and dangling views", () => {
    const m: any = weather().manifest();
    m.tools[0].annotations = {};
    m.tools[1].view = "ui://nope/x.html";
    const codes = validateManifest(m).map((x) => x.code);
    expect(codes).toEqual(expect.arrayContaining(["tool.annotations-missing", "resource.tool-target-missing"]));
  });

  it("lints clean for the demo, and catches a write-named tool marked read-only", () => {
    expect(hasErrors(weather().lint())).toBe(false);
    const app = defineApp({
      name: "W",
      tools: [appTool({ name: "delete_item", title: "Delete", description: "Delete an item permanently.", annotations: { readOnlyHint: true, destructiveHint: false }, handler: async () => ({ content: [] }) })],
    });
    expect(app.lint().map((x) => x.code)).toContain("tool.readonly-implausible");
  });
});

describe("toPluginFiles", () => {
  it("builds plugin.json and mcp.json in the layout plugin-package reads", () => {
    const files = toPluginFiles(weather().manifest(), "https://weather.example.com/mcp");
    expect(files["plugin.json"]).toMatchObject({ name: "Weather demo", version: "1.2.3" });
    expect(files["mcp.json"].mcpServers["weather-demo"]).toEqual({ type: "streamable-http", url: "https://weather.example.com/mcp" });
  });

  it("refuses a missing or non-https url", () => {
    expect(() => toPluginFiles(weather().manifest())).toThrow(/https/);
    expect(() => toPluginFiles(weather().manifest(), "http://x.test/mcp")).toThrow(/https/);
  });
});

describe("listen (real MCP round trip)", () => {
  let running: RunningApp | undefined;
  afterEach(async () => {
    await running?.close();
    running = undefined;
  });

  for (const mode of ["legacy", { pin: "2026-07-28" }] as const) {
    it(`serves tools with _meta.ui.resourceUri, the View resource and the manifest (${JSON.stringify(mode)} client)`, async () => {
      running = await listen(weather(), { port: 0 });
      const client = new Client({ name: "check", version: "1.0.0" }, { versionNegotiation: { mode } });
      await client.connect(new StreamableHTTPClientTransport(new URL(running.url)));
      expect(client.getProtocolEra()).toBe(mode === "legacy" ? "legacy" : "modern");

      const { tools } = await client.listTools();
      expect(tools.map((t) => t.name)).toEqual(["get_temperature", "show_weather"]);
      const show = tools.find((t) => t.name === "show_weather")!;
      expect((show._meta as any).ui.resourceUri).toBe("ui://weather/card.html");
      expect(show.annotations).toMatchObject({ readOnlyHint: true, destructiveHint: false });
      expect(tools.find((t) => t.name === "get_temperature")!._meta).toBeUndefined();

      const res: any = await client.callTool({ name: "show_weather", arguments: { city: "Tokyo" } });
      expect(res.structuredContent).toEqual({ city: "Tokyo", temperatureC: 22 });

      const read = await client.readResource({ uri: "ui://weather/card.html" });
      expect(read.contents[0].mimeType).toBe("text/html;profile=mcp-app");
      expect(String((read.contents[0] as any).text)).toContain("allternit-detail");
      expect((read.contents[0]._meta as any).ui.csp).toEqual({ connectDomains: [], resourceDomains: [] });
      await client.close();

      const base = new URL(running.url).origin;
      expect(await (await fetch(`${base}/healthz`)).text()).toBe("ok");
      expect((await (await fetch(`${base}/allternit.app.json`)).json()).schema).toBe("allternit.app/1");
      expect((await fetch(`${base}/nope`)).status).toBe(404);
    });
  }
});

const META = {
  "io.modelcontextprotocol/protocolVersion": "2026-07-28",
  "io.modelcontextprotocol/clientInfo": { name: "smoke", version: "1" },
  "io.modelcontextprotocol/clientCapabilities": {},
};

async function rpc(handler: ReturnType<typeof createHandler>, method: string, params: Record<string, unknown>, era: "modern" | "legacy", id = 1) {
  const headers: Record<string, string> = { "content-type": "application/json", accept: "application/json, text/event-stream" };
  if (era === "modern") Object.assign(headers, { "mcp-protocol-version": "2026-07-28", "mcp-method": method }, params.name ? { "mcp-name": String(params.name) } : {});
  const body = { jsonrpc: "2.0", id, method, params: era === "modern" ? { ...params, _meta: META } : params };
  const res = await handler.fetch(new Request("http://app.test/mcp", { method: "POST", headers, body: JSON.stringify(body) }));
  const text = await res.text();
  const json = text.startsWith("{") ? text : text.split("\n").find((l) => l.startsWith("data: "))!.slice(6);
  return JSON.parse(json);
}

describe("dual-era smoke (raw wire)", () => {
  it("answers server/discover and tools/list with _meta (2026-07-28)", async () => {
    const h = createHandler(weather());
    const discover = await rpc(h, "server/discover", {}, "modern");
    expect(discover.result.supportedVersions).toContain("2026-07-28");
    expect(discover.result.capabilities.tools).toBeDefined();
    expect(discover.result.resultType).toBe("complete");
    const list = await rpc(h, "tools/list", {}, "modern", 2);
    expect(list.result.tools.map((t: any) => t.name)).toEqual(["get_temperature", "show_weather"]);
    expect(list.result._meta["io.modelcontextprotocol/serverInfo"].name).toBe("weather-demo");
    await h.close();
  });

  it("answers legacy initialize and tools/list", async () => {
    const h = createHandler(weather());
    const init = await rpc(h, "initialize", { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "old", version: "1" } }, "legacy");
    expect(init.result.protocolVersion).toBe("2025-06-18");
    const list = await rpc(h, "tools/list", {}, "legacy", 2);
    expect(list.result.tools.map((t: any) => t.name)).toEqual(["get_temperature", "show_weather"]);
    await h.close();
  });
});

describe("events", () => {
  const secret = `whsec_${Buffer.alloc(32, 9).toString("base64")}`;
  const subs = new Map<string, unknown>();
  const app = () =>
    defineApp({
      name: "Alerts",
      tools: [appTool({ name: "ping", title: "Ping", description: "Answers pong to check the app.", annotations: { readOnlyHint: true, destructiveHint: false }, handler: async () => ({ content: [{ type: "text", text: "pong" }] }) })],
      events: {
        definitions: [
          { name: "price.changed", description: "A price moved.", delivery: ["webhook"], inputSchema: { type: "object" }, payloadSchema: { type: "object" } },
          { name: "alert.fired", description: "An alert fired.", delivery: ["webhook"], inputSchema: { type: "object" }, payloadSchema: { type: "object" } },
        ],
        async subscribe(sub) {
          if (sub.arguments.symbol === "NOPE") throw eventsError(EventsErrorCode.Forbidden, "not yours");
          subs.set(sub.id, sub);
          return { refreshBefore: null, cursor: "c0" };
        },
        async unsubscribe(sub) {
          subs.delete(sub.id);
        },
      },
    });

  it("rejects bad definitions at defineApp", () => {
    const t = appTool({ name: "x", title: "X", description: "Does x for the user.", annotations: { readOnlyHint: true, destructiveHint: false }, handler: async () => ({ content: [] }) });
    const def = { name: "a", description: "a", delivery: ["webhook" as const], inputSchema: {}, payloadSchema: {} };
    const noop = { subscribe: async () => ({ refreshBefore: null, cursor: null }), unsubscribe: async () => {} };
    expect(() => defineApp({ name: "A", tools: [t], events: { definitions: [def, def], ...noop } })).toThrow(/duplicate event/);
    expect(() => defineApp({ name: "A", tools: [t], events: { definitions: [{ ...def, delivery: ["poll"] }], ...noop } })).toThrow(/webhook/);
  });

  for (const era of ["modern", "legacy"] as const) {
    it(`serves events/list|subscribe|unsubscribe (${era})`, async () => {
      subs.clear();
      const h = createHandler(app());
      if (era === "legacy") await rpc(h, "initialize", { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "old", version: "1" } }, "legacy");
      else expect((await rpc(h, "server/discover", {}, "modern")).result.capabilities.events).toEqual({ listChanged: false });
      const list = await rpc(h, "events/list", {}, era, 2);
      expect(list.result.events.map((e: any) => e.name)).toEqual(["alert.fired", "price.changed"]);
      const params = { name: "price.changed", arguments: { symbol: "ACME" }, delivery: { mode: "webhook", url: "https://cb.example.com/1", secret } };
      const sub = await rpc(h, "events/subscribe", params, era, 3);
      expect(sub.result).toMatchObject({ refreshBefore: null, cursor: "c0", truncated: false });
      expect(sub.result.id).toMatch(/^sub_[0-9a-f]{24}$/);
      expect(subs.has(sub.result.id)).toBe(true);
      const again = await rpc(h, "events/subscribe", { ...params, arguments: { symbol: "ACME" } }, era, 4);
      expect(again.result.id).toBe(sub.result.id);
      expect((await rpc(h, "events/subscribe", { ...params, name: "nope" }, era, 5)).error.code).toBe(-32011);
      expect((await rpc(h, "events/subscribe", { ...params, arguments: { symbol: "NOPE" } }, era, 6)).error.code).toBe(-32012);
      expect((await rpc(h, "events/subscribe", { ...params, delivery: { ...params.delivery, url: "https://10.0.0.1/x" } }, era, 7)).error.code).toBe(-32602);
      expect((await rpc(h, "events/subscribe", { ...params, delivery: { mode: "poll" } }, era, 8)).error.code).toBe(-32014);
      const { secret: _s, ...noSecret } = params.delivery;
      expect((await rpc(h, "events/unsubscribe", { name: params.name, arguments: params.arguments, delivery: noSecret }, era, 9)).result).toEqual(expect.objectContaining({}));
      expect(subs.size).toBe(0);
      await h.close();
    });
  }

});
