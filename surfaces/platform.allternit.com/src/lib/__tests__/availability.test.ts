import { describe, expect, it } from "vitest";
import { fabricEnabled, isCloudHost } from "../availability";
import { buildConsoleNav, buildConsoleNavEntries } from "@/components/console-ui/navConfig";

const labels = (cloud: boolean, fabric: boolean) =>
  buildConsoleNavEntries(buildConsoleNav({ cloud, fabric })).map((e) => e.label);
// Labels repeat across groups (Platform API / Computers, Managed Agents /
// Computers); the rail, palette and breadcrumb always show them with the group.
const qualified = (cloud: boolean, fabric: boolean) =>
  buildConsoleNavEntries(buildConsoleNav({ cloud, fabric })).map((e) =>
    e.group ? `${e.group} / ${e.label}` : e.label
  );

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
  const hidden = [
    "Batches",
    "Files",
    "Spend limits",
    "Service accounts",
    "Caching",
    "Tags",
    "Organization webhooks",
    "Routing policy",
    "Route credentials",
  ];

  it("drops unavailable items on cloud and Fabric without a URL", () => {
    const l = labels(true, false);
    for (const h of hidden) expect(l).not.toContain(h);
    expect(l).not.toContain("Fabric");
  });

  it("drops the Gateway group on cloud and keeps Audit log and Project webhooks", () => {
    const nav = buildConsoleNav({ cloud: true, fabric: false });
    expect(nav.groups.map((g) => g.label)).not.toContain("Gateway");
    const l = qualified(true, false);
    expect(l).toContain("Organization / Audit log");
    expect(l).toContain("Platform API / Webhooks");
  });

  it("keeps cloud entries unique within their group", () => {
    const l = qualified(true, true);
    expect(new Set(l).size).toBe(l.length);
  });

  it("keeps Security on cloud as the organization policy page", () => {
    expect(labels(true, false)).toContain("Security");
  });

  it("keeps runtime items and unique entries within their group", () => {
    const l = labels(false, true);
    for (const h of [...hidden, "Security"]) expect(l).toContain(h);
    expect(l).toContain("Fabric");
    const q = qualified(false, true);
    expect(new Set(q).size).toBe(q.length);
  });

  it("tells the overlapping pages apart by group and keeps one Rate limits entry", () => {
    const q = qualified(false, true);
    for (const n of [
      "Platform API / Computers",
      "Managed Agents / Computers",
      "Platform API / Webhooks",
      "Organization / Organization webhooks",
      "Platform API / Usage",
      "Analytics / Usage",
      "Platform API / Billing",
      "Organization / Credits & billing",
    ])
      expect(q).toContain(n);
    expect(labels(false, true).filter((x) => x === "Rate limits")).toHaveLength(1);
  });
});
