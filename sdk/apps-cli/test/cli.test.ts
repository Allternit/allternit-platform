import { mkdirSync, mkdtempSync, readFileSync, writeFileSync, existsSync, cpSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import JSZip from "jszip";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { parseArgs } from "../src/args.js";
import { run, type Context } from "../src/index.js";
import { validateSubmission } from "../src/index.js";

let root: string;
const logs: string[] = [];
const errors: string[] = [];
const requests: Array<{ url: string; init: RequestInit }> = [];

const ctxFor = (cwd: string, env: Record<string, string> = {}, fetchImpl?: typeof fetch): Context => ({
  cwd,
  env,
  log: (l) => logs.push(l),
  error: (l) => errors.push(l),
  fetch: fetchImpl ?? (async () => new Response("{}")),
});

beforeAll(() => {
  root = mkdtempSync(join(tmpdir(), "allternit-cli-"));
});
afterAll(() => undefined);

describe("parseArgs", () => {
  it("handles values, =, booleans and positionals", () => {
    const p = parseArgs(["a", "--url", "https://x", "--out=f.zip", "--strict", "b"], ["strict"]);
    expect(p.positional).toEqual(["a", "b"]);
    expect(p.flags).toEqual({ url: "https://x", out: "f.zip", strict: true });
  });
});

describe("create", () => {
  it("scaffolds the template with the name substituted and refuses a non-empty dir", async () => {
    expect(await run(ctxFor(root), ["create", "My Weather App"])).toBe(0);
    const dir = join(root, "My Weather App");
    const server = readFileSync(join(dir, "server.ts"), "utf8");
    expect(server).toContain('uri: "ui://my-weather-app/card.html"');
    expect(server).not.toContain("__APP_");
    expect(JSON.parse(readFileSync(join(dir, "package.json"), "utf8")).name).toBe("my-weather-app");
    expect(existsSync(join(dir, ".gitignore"))).toBe(true);
    errors.length = 0;
    expect(await run(ctxFor(root), ["create", "My Weather App"])).toBe(1);
    expect(errors.join()).toMatch(/not empty/);
  });

  it("needs a name", async () => {
    errors.length = 0;
    expect(await run(ctxFor(root), ["create"])).toBe(1);
    expect(errors.join()).toMatch(/Usage/);
  });
});

describe("test / package / submit on the scaffolded app", () => {
  let dir: string;
  beforeAll(async () => {
    await run(ctxFor(root), ["create", "demo"]);
    dir = join(root, "demo");
    // Resolve the template's SDK import to the workspace build.
    const nm = join(dir, "node_modules/@allternit");
    mkdirSync(nm, { recursive: true });
    const { symlinkSync } = await import("node:fs");
    symlinkSync(resolve(__dirname, "../../apps-server"), join(nm, "apps-server"), "dir");
  });

  it("test passes the template with no errors (declared lint + live MCP round trip)", async () => {
    logs.length = 0;
    const code = await run(ctxFor(dir), ["test"]);
    expect(code, logs.join("\n") + errors.join("\n")).toBe(0);
    expect(logs.join("\n")).toContain("PASSED");
  });

  it("test fails a server whose tool lacks annotations", async () => {
    const bad = join(root, "bad");
    mkdirSync(bad, { recursive: true });
    mkdirSync(join(bad, "node_modules/@allternit"), { recursive: true });
    const { symlinkSync } = await import("node:fs");
    symlinkSync(resolve(__dirname, "../../apps-server"), join(bad, "node_modules/@allternit/apps-server"), "dir");
    writeFileSync(
      join(bad, "server.ts"),
      `import { defineApp } from "@allternit/apps-server";
export default defineApp({ name: "Bad", tools: [{ name: "x", title: "X", description: "Does something.", annotations: {}, handler: async () => ({ content: [] }) } as any] });`,
    );
    logs.length = 0;
    errors.length = 0;
    expect(await run(ctxFor(bad), ["test"])).toBe(1);
    expect(logs.join("\n")).toContain("FAILED");
    expect(logs.join("\n")).toContain("tool.annotations-missing");
  });

  it("package needs a public https url, then writes a zip the plugin parser's layout accepts", async () => {
    errors.length = 0;
    expect(await run(ctxFor(dir), ["package"])).toBe(1);
    expect(errors.join()).toMatch(/--url/);
    errors.length = 0;
    expect(await run(ctxFor(dir), ["package", "--url", "http://x.test/mcp"])).toBe(1);

    expect(await run(ctxFor(dir), ["package", "--url", "https://demo.example.com/mcp"])).toBe(0);
    const zip = await JSZip.loadAsync(readFileSync(join(dir, "demo.zip")));
    expect(Object.keys(zip.files).sort()).toEqual(["allternit.app.json", "mcp.json", "plugin.json", "skills/", "skills/demo/", "skills/demo/SKILL.md"].filter((n) => zip.files[n]).sort());
    const mcp = JSON.parse(await zip.file("mcp.json")!.async("string"));
    expect(Object.keys(mcp.mcpServers)).toHaveLength(1);
    expect(mcp.mcpServers.demo).toEqual({ type: "streamable-http", url: "https://demo.example.com/mcp" });
    const plugin = JSON.parse(await zip.file("plugin.json")!.async("string"));
    expect(plugin.name.length).toBeLessThanOrEqual(30);
    const skill = await zip.file("skills/demo/SKILL.md")!.async("string");
    expect(skill).toMatch(/^---\s*\n[\s\S]*?\n---/);
    expect(skill).toMatch(/^description:/m);
    const manifest = JSON.parse(await zip.file("allternit.app.json")!.async("string"));
    expect(manifest.server.url).toBe("https://demo.example.com/mcp");
    expect(JSON.stringify(manifest)).not.toMatch(/secret|password|api[-_]?key|bearer/i);
  });

  it("submit refuses without the listing file, and validates the form before any network call", async () => {
    errors.length = 0;
    expect(await run(ctxFor(dir), ["submit", "demo.zip", "--no-scan"])).toBe(1);
    expect(errors.join()).toMatch(/allternit\.submission\.json not found/);

    writeFileSync(join(dir, "allternit.submission.json"), JSON.stringify({ developer: "Me" }));
    errors.length = 0;
    logs.length = 0;
    expect(await run(ctxFor(dir), ["submit", "demo.zip", "--no-scan"])).toBe(1);
    expect(logs.join("\n")).toContain("form.privacy-url");
    expect(errors.join()).toMatch(/Fix the listing/);
  });

  it("submit --dry-run redacts credentials and sends nothing; a real submit posts base64 with a bearer token", async () => {
    const tests = (n: number) => Array.from({ length: n }, (_, i) => ({ prompt: `p${i}`, expected: `e${i}` }));
    writeFileSync(
      join(dir, "allternit.submission.json"),
      JSON.stringify({
        longDescription: "Weather.",
        developer: "Me",
        privacyUrl: "https://demo.example.com/privacy",
        termsUrl: "https://demo.example.com/terms",
        icon: "https://demo.example.com/icon.png",
        screenshots: ["https://demo.example.com/s.png"],
        positiveTests: tests(5),
        negativeTests: tests(3),
        reviewerCredentials: "hunter2",
      }),
    );
    requests.length = 0;
    const fetchImpl = (async (url: string, init: RequestInit) => {
      requests.push({ url, init });
      return new Response(JSON.stringify({ id: "sub_1", state: "in_review" }));
    }) as unknown as typeof fetch;

    logs.length = 0;
    expect(await run(ctxFor(dir, {}, fetchImpl), ["submit", "demo.zip", "--no-scan", "--dry-run"])).toBe(0);
    expect(requests).toHaveLength(0);
    expect(logs.join("\n")).not.toContain("hunter2");
    expect(logs.join("\n")).toContain("<redacted>");

    errors.length = 0;
    expect(await run(ctxFor(dir, {}, fetchImpl), ["submit", "demo.zip", "--no-scan"])).toBe(1);
    expect(errors.join()).toMatch(/token is required/);

    logs.length = 0;
    expect(await run(ctxFor(dir, { ALLTERNIT_TOKEN: "tok_123" }, fetchImpl), ["submit", "demo.zip", "--no-scan"])).toBe(0);
    expect(requests).toHaveLength(1);
    expect(requests[0].url).toBe("https://api.allternit.com/api/v1/miniapps/submissions");
    expect((requests[0].init.headers as Record<string, string>).authorization).toBe("Bearer tok_123");
    const body = JSON.parse(String(requests[0].init.body));
    expect(body.name).toBe("demo");
    expect(Buffer.from(body.packageZip, "base64").subarray(0, 2).toString()).toBe("PK");
    expect(logs.join("\n")).not.toContain("tok_123");
  });

  it("dev starts the server, prints connect instructions and answers MCP", async () => {
    logs.length = 0;
    const { dev } = await import("../src/index.js");
    const running = await dev(ctxFor(dir), ["--port", "0"]);
    try {
      expect(logs.join("\n")).toContain("Add it as a connector");
      expect(logs.join("\n")).toContain("/mcp");
      expect((await fetch(`${new URL(running.url).origin}/healthz`)).status).toBe(200);
    } finally {
      await running.close();
    }
  });
});

describe("validateSubmission", () => {
  it("rejects private hosts and over-long fields", () => {
    const codes = validateSubmission({ name: "x".repeat(31), privacyUrl: "https://localhost/p", termsUrl: "http://x.test" }).map((f) => f.code);
    expect(codes).toEqual(expect.arrayContaining(["form.name-too-long", "form.privacy-url", "form.terms-url"]));
  });
});

describe("run", () => {
  it("prints help and rejects unknown commands", async () => {
    logs.length = 0;
    expect(await run(ctxFor(root), [])).toBe(0);
    expect(logs.join()).toContain("allternit <command>");
    errors.length = 0;
    expect(await run(ctxFor(root), ["nope"])).toBe(1);
  });
});
