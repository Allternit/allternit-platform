import { describe, expect, it } from "vitest";
import {
  buildConsoleNav,
  buildConsoleNavEntries,
  consoleNavEntryForPath,
} from "@/components/console-ui/navConfig";

// Every sidebar URL on origin/main before the 2026-11-13 rail regroup
// (a07fb7b2d). The regroup moved pages between groups; none may be lost.
// /docs moved to the rail footer link line, so it is not in the nav.
const URLS_BEFORE_REGROUP = [
  "/", "/api-keys", "/playground", "/files", "/skills", "/batches",
  "/platform/projects", "/platform/agents", "/platform/conversations",
  "/platform/computers", "/platform/phones", "/platform/usage",
  "/platform/billing", "/platform/webhooks", "/agents", "/builder", "/sessions",
  "/deployments", "/computers", "/vaults", "/memory", "/analytics/usage",
  "/analytics/logs", "/analytics/caching", "/analytics/rate-limits",
  "/analytics/cost", "/gizzi/usage", "/gateway/routing-policy",
  "/gateway/credentials", "/manage/spend-limits", "/manage/members",
  "/manage/service-accounts", "/manage/security", "/manage/audit-log",
  "/manage/webhooks", "/manage/tags", "/organizations", "/compute", "/devices",
  "/fabric", "/runs", "/schedules", "/approvals", "/cloud-accounts", "/billing",
  "/models", "/plans", "/settings",
];

describe("console nav regroup", () => {
  const entries = buildConsoleNavEntries(buildConsoleNav({ cloud: false, fabric: true }));
  const urls = entries.map((e) => e.to);

  it("keeps every pre-regroup URL exactly once", () => {
    for (const url of URLS_BEFORE_REGROUP) {
      expect({ url, count: urls.filter((u) => u === url).length }).toEqual({ url, count: 1 });
    }
    expect(urls).not.toContain("/docs");
  });

  it("resolves nested pages to their own group for the breadcrumb", () => {
    expect(consoleNavEntryForPath("/platform/agents/abc", entries)?.group).toBe("Platform API");
    expect(consoleNavEntryForPath("/agents/new", entries)?.group).toBe("Managed Agents");
    expect(consoleNavEntryForPath("/", entries)?.label).toBe("Dashboard");
    expect(consoleNavEntryForPath("/", entries)?.group).toBeNull();
    expect(consoleNavEntryForPath("/no-such-page", entries)).toBeUndefined();
  });
});
