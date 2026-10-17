import { describe, expect, it } from "vitest";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { DesktopPhoneGateway, PHONE_GATEWAY_TOKEN_HEADER } from "../src/phone/gateway.js";
import { phoneRouteForAdapterId, routesForAgentManifest } from "../src/routes/registry.js";
import { parseUiTree, nodeText } from "../adapters/_shared/phone/ui-tree.js";
import { PhoneChatProvider } from "../adapters/_shared/phone/chat-provider.js";
import { ReplayPhoneGateway, type ReplayMode } from "../adapters/_shared/phone/replay.js";
import { CHATGPT_ANDROID, CHATGPT_IPHONE } from "../adapters/_shared/phone/chatgpt.js";

const fast = { pollMs: 1, settleMs: 0, replyTimeoutMs: 2000, sleep: () => Promise.resolve() };
const provider = (spec = CHATGPT_IPHONE, mode: ReplayMode = "normal") => {
  const gateway = new ReplayPhoneGateway(spec, mode);
  return { gateway, p: new PhoneChatProvider(spec, { gateway, ...fast }) };
};

describe("parseUiTree (real read_ui lines, 2026-10-17)", () => {
  it("reads iPhone value/label/id/frame and Android flags", () => {
    const nodes = parseUiTree([
      "Application, label: 'ChatGPT', {{0, 0}, {393, 852}}",
      "  StaticText, value: 'Allternit iphone tree works', {{16, 229}, {361, 24}}",
      "  Button, label: 'Send', id: 'MessageInputPrimaryButtonContainerPart.PrimaryButton', {{333, 457}, {48, 48}}",
      "    EditText, {{106, 1411}, {593, 84}}, clickable long-clickable editable",
      "  TextView, label: 'It\\'s here', {{1, 2}, {3, 4}}",
    ].join("\n"));
    expect(nodes[1]).toMatchObject({ depth: 2, cls: "StaticText", value: "Allternit iphone tree works", frame: { x: 16, y: 229, w: 361, h: 24 } });
    expect(nodes[2]).toMatchObject({ cls: "Button", label: "Send", id: "MessageInputPrimaryButtonContainerPart.PrimaryButton" });
    expect(nodes[3]).toMatchObject({ depth: 4, cls: "EditText", flags: ["clickable", "long-clickable", "editable"] });
    expect(nodeText(nodes[4])).toBe("It's here");
  });
});

describe("routes", () => {
  it("phone adapters map to phone routes by id; desktop adapters keep both desktop lanes", () => {
    expect(phoneRouteForAdapterId("chatgpt-android-app")).toBe("android-app");
    expect(phoneRouteForAdapterId("chatgpt-iphone-app")).toBe("iphone-app-ictrl");
    expect(phoneRouteForAdapterId("claude-desktop")).toBeNull();
    expect(routesForAgentManifest(CHATGPT_ANDROID.capabilities)).toEqual(["android-app"]);
    expect(routesForAgentManifest(CHATGPT_IPHONE.capabilities)).toEqual(["iphone-app-ictrl"]);
  });
});

describe("DesktopPhoneGateway", () => {
  it("posts {name, arguments} with the token header read from Desktop's file", async () => {
    const dir = mkdtempSync(join(tmpdir(), "phone-gw-"));
    const file = join(dir, "phone-gateway.json");
    writeFileSync(file, JSON.stringify({ url: "http://127.0.0.1:5555/", token: "t0k" }));
    const seen: { url: string; init: RequestInit }[] = [];
    const fetchImpl = (async (url: string, init: RequestInit) => {
      seen.push({ url, init });
      return new Response(JSON.stringify({ ok: true, devices: [] }), { status: 200 });
    }) as unknown as typeof fetch;
    const r = await new DesktopPhoneGateway({ file, fetchImpl }).call("phone.read_ui", { serial: "link:x" });
    expect(r).toEqual({ ok: true, devices: [] });
    expect(seen[0].url).toBe("http://127.0.0.1:5555/phone/call");
    expect((seen[0].init.headers as Record<string, string>)[PHONE_GATEWAY_TOKEN_HEADER]).toBe("t0k");
    expect(JSON.parse(String(seen[0].init.body))).toEqual({ name: "phone.read_ui", arguments: { serial: "link:x" } });
  });

  it("answers desktop_unavailable when Desktop's gateway file is missing", async () => {
    const r = await new DesktopPhoneGateway({ file: join(tmpdir(), "nope", "missing.json") }).call("phone.devices");
    expect(r).toMatchObject({ ok: false, error: "desktop_unavailable" });
  });
});

describe.each([
  ["iPhone", CHATGPT_IPHONE],
  ["Android", CHATGPT_ANDROID],
])("ChatGPT on %s (replayed screens)", (_name, spec) => {
  it("opens the app, sends, waits for the stop control to go, returns the reply", async () => {
    const { gateway, p } = provider(spec);
    const open = await p.contextOpen({ agentId: spec.agentId });
    expect(open.ok).toBe(true);
    if (!open.ok) return;
    const msg = await p.contextMessage({ contextId: open.value.contextId, correlationId: "c1", text: "Say hi" });
    expect(msg).toMatchObject({ ok: true, value: { reply: "Reply to: Say hi" } });
    const names = gateway.calls.map((c) => c.name);
    expect(names).toContain("phone.open_app");
    expect(gateway.calls.find((c) => c.name === "phone.open_app")?.args).toMatchObject({ app: spec.appId });
    expect(gateway.calls.filter((c) => c.name === "phone.find_and_tap").map((c) => c.args.element_id)).toEqual([spec.composer, expect.stringMatching(spec.send)]);
    const ev = await p.events({ contextId: open.value.contextId });
    expect(ev.ok && ev.value.events.map((e) => e.event.type)).toEqual(["agent.context.opened", "agent.activity.started", "agent.message.delta", "agent.message.completed"]);
  });

  it("a second message only returns the new reply and replays by correlation id", async () => {
    const { p } = provider(spec);
    const open = await p.contextOpen({ agentId: spec.agentId });
    if (!open.ok) throw new Error("open failed");
    await p.contextMessage({ contextId: open.value.contextId, correlationId: "a", text: "one" });
    const two = await p.contextMessage({ contextId: open.value.contextId, correlationId: "b", text: "two" });
    expect(two).toMatchObject({ ok: true, value: { reply: "Reply to: two" } });
    const again = await p.contextMessage({ contextId: open.value.contextId, correlationId: "b", text: "two" });
    expect(again).toEqual(two);
  });

  it.each([
    ["logged_out", "AUTH_REQUIRED"],
    ["rate_limited", "RATE_LIMITED"],
    ["verification", "LANE_BLOCKED"],
    ["down", "VENDOR_UNAVAILABLE"],
    ["locked", "VENDOR_UNAVAILABLE"],
    ["no_phone", "VENDOR_UNAVAILABLE"],
  ] as const)("%s → typed %s", async (mode, code) => {
    const { p } = provider(spec, mode);
    const open = await p.contextOpen({ agentId: spec.agentId });
    expect(open.ok).toBe(false);
    if (!open.ok) expect(open.error.code).toBe(code);
  });

  it("a missing composer latches drift at open and stops driving", async () => {
    const { p } = provider(spec, "drift");
    const open = await p.contextOpen({ agentId: spec.agentId });
    expect(open.ok === false && open.error.code).toBe("ADAPTER_DRIFT");
    const next = await p.contextOpen({ agentId: spec.agentId });
    expect(next.ok === false && next.error.code).toBe("ADAPTER_DRIFT");
  });

  it("health is down with the phone's own message when it isn't linked", async () => {
    const { p } = provider(spec, "no_phone");
    const h = await p.health({});
    expect(h).toMatchObject({ ok: true, value: { status: "down" } });
  });
});
