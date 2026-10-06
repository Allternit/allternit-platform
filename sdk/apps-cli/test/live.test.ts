import { afterEach, describe, expect, it } from "vitest";
import { appTool, defineApp, listen, view, type RunningApp } from "@allternit/apps-server";
import { liveScan } from "../src/live.js";

describe("liveScan against a dual-era app server", () => {
  let running: RunningApp | undefined;
  afterEach(async () => {
    await running?.close();
    running = undefined;
  });

  it("negotiates, lists tools in order and reads the ui:// view", async () => {
    const card = view({ uri: "ui://demo/card.html", name: "Card", kit: { tag: "allternit-card" } });
    const app = defineApp({
      name: "Demo",
      tools: [
        appTool({ name: "zeta", title: "Zeta", description: "Shows the zeta card to the user.", annotations: { readOnlyHint: true, destructiveHint: false }, view: card, handler: async () => ({ content: [] }) }),
        appTool({ name: "alpha", title: "Alpha", description: "Reads the alpha value for the user.", annotations: { readOnlyHint: true, destructiveHint: false }, handler: async () => ({ content: [] }) }),
      ],
      views: [card],
    });
    running = await listen(app, { port: 0 });
    const { findings, input } = await liveScan(running.url);
    expect(input.tools.map((t) => t.name)).toEqual(["alpha", "zeta"]);
    expect(input.resources.map((r) => r.uri)).toEqual(["ui://demo/card.html"]);
    expect(findings.filter((f) => f.severity === "error")).toEqual([]);
  });
});
