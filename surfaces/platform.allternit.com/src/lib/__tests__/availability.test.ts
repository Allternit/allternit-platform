import { describe, expect, it } from "vitest";
import { fabricEnabled, isCloudHost } from "../availability";
import { buildConsoleNav, buildConsoleNavEntries } from "@/components/console-ui/navConfig";

const labels = (cloud: boolean, fabric: boolean) =>
  buildConsoleNavEntries(buildConsoleNav({ cloud, fabric })).map((e) => e.label);

describe("isCloudHost", () => {
  it("detects the cloud gateway, including the default", () => {
    expect(isCloudHost(undefined)).toBe(true);
    expect(isCloudHost("https://api.allternit.com/")).toBe(true);
    expect(isCloudHost("http://127.0.0.1:3210")).toBe(false);
    expect(isCloudHost("not a url")).toBe(false);
  });
});

describe("fabricEnabled", () => {
  it("returns a boolean", () => {
    expect(typeof fabricEnabled()).toBe("boolean");
  });
});

describe("console nav", () => {
  const hidden = ["Batches", "Files", "Spend limits", "Service accounts", "Security", "Gizzi Code usage"];

  it("drops unavailable items on cloud and Fabric without a URL", () => {
    const l = labels(true, false);
    for (const h of hidden) expect(l).not.toContain(h);
    expect(l).not.toContain("Fabric");
  });

  it("keeps cloud labels unique", () => {
    const l = labels(true, true);
    expect(new Set(l).size).toBe(l.length);
  });

  it("keeps runtime items and unique labels", () => {
    const l = labels(false, true);
    for (const h of hidden) expect(l).toContain(h);
    expect(l).toContain("Fabric");
    expect(new Set(l).size).toBe(l.length);
  });

  it("uses the renamed labels and one Rate limits entry", () => {
    const l = labels(false, true);
    for (const n of ["Hosted computers", "Agent computers", "Project webhooks", "Organization webhooks", "Project usage", "Project billing"])
      expect(l).toContain(n);
    expect(l.filter((x) => x === "Rate limits")).toHaveLength(1);
  });
});
