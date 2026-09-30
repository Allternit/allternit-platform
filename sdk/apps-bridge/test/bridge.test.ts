// @vitest-environment happy-dom
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AppsBridge, METHODS, shouldInjectOpenAiCompat, SKYBRIDGE_MIME_TYPE } from "../src/index.js";
import { allternitExtSource, openaiCompatSource } from "../src/sources.js";

function setup() {
  const sent: any[] = [];
  const host = { postMessage: (m: unknown) => sent.push(m) };
  const bridge = new AppsBridge({ appInfo: { name: "t", version: "1" }, target: host as any });
  const fromHost = (data: unknown) => window.dispatchEvent(new MessageEvent("message", { data, source: host as any }));
  return { sent, bridge, fromHost };
}

describe("AppsBridge", () => {
  beforeEach(() => document.documentElement.removeAttribute("style"));

  it("runs the initialize handshake, applies host variables and announces itself", async () => {
    const { sent, bridge, fromHost } = setup();
    const connecting = bridge.connect();
    expect(sent[0]).toMatchObject({ method: METHODS.initialize, params: { appInfo: { name: "t" } } });
    fromHost({ jsonrpc: "2.0", id: sent[0].id, result: { hostContext: { theme: "dark", styles: { variables: { "--vp-color-brand": "#123456", nope: "x" } } } } });
    const ctx = await connecting;
    expect(ctx.theme).toBe("dark");
    expect(document.documentElement.style.getPropertyValue("--vp-color-brand")).toBe("#123456");
    expect(document.documentElement.style.getPropertyValue("nope")).toBe("");
    expect(sent[1]).toMatchObject({ method: METHODS.initialized });
  });

  it("delivers tool results to listeners and ignores other windows", () => {
    const { bridge, fromHost } = setup();
    const fn = vi.fn();
    bridge.onToolResult(fn);
    fromHost({ jsonrpc: "2.0", method: METHODS.toolResult, params: { structuredContent: { a: 1 } } });
    window.dispatchEvent(new MessageEvent("message", { data: { jsonrpc: "2.0", method: METHODS.toolResult, params: {} }, source: window }));
    expect(fn).toHaveBeenCalledTimes(1);
    expect(fn.mock.calls[0][0].structuredContent).toEqual({ a: 1 });
  });

  it("rejects a request on a JSON-RPC error and resolves on a result", async () => {
    const { sent, bridge, fromHost } = setup();
    const ok = bridge.callTool("get", { x: 1 });
    const bad = bridge.callTool("boom");
    expect(sent[0].params).toEqual({ name: "get", arguments: { x: 1 } });
    fromHost({ jsonrpc: "2.0", id: sent[0].id, result: { content: [] } });
    fromHost({ jsonrpc: "2.0", id: sent[1].id, error: { code: -32000, message: "nope" } });
    await expect(ok).resolves.toEqual({ content: [] });
    await expect(bad).rejects.toMatchObject({ message: "nope", code: -32000 });
  });

  it("answers teardown so the host can close the View", () => {
    const { sent, fromHost } = setup();
    fromHost({ jsonrpc: "2.0", id: 9, method: METHODS.teardown, params: {} });
    expect(sent).toContainEqual({ jsonrpc: "2.0", id: 9, result: {} });
  });

  it("rejects in-flight requests on dispose", async () => {
    const { bridge } = setup();
    const p = bridge.callTool("x");
    bridge.dispose();
    await expect(p).rejects.toThrow(/disposed/);
  });
});

describe("openai compat + runtime sources", () => {
  it("opts in on skybridge MIME or openai/* metadata only", () => {
    expect(shouldInjectOpenAiCompat({ mimeType: SKYBRIDGE_MIME_TYPE })).toBe(true);
    expect(shouldInjectOpenAiCompat({ toolMeta: { "openai/outputTemplate": "x" } })).toBe(true);
    expect(shouldInjectOpenAiCompat({ resourceMeta: { ui: {} } })).toBe(false);
  });

  it("ships both runtime scripts dependency-free", () => {
    for (const src of [allternitExtSource(), openaiCompatSource()]) {
      expect(src).not.toMatch(/\bimport\s|\brequire\(/);
    }
    expect(allternitExtSource()).toContain("window.allternit");
    expect(openaiCompatSource()).toContain("window.openai");
  });

  it("window.allternit installs in a framed View and does nothing at top level", () => {
    const w: any = { parent: {}, addEventListener() {} };
    new Function("window", allternitExtSource()).call(w, w);
    expect(typeof w.allternit.detect).toBe("function");
    const top: any = {};
    top.parent = top;
    new Function("window", allternitExtSource()).call(top, top);
    expect(top.allternit).toBeUndefined();
  });
});
