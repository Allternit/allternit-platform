import { describe, expect, it } from "vitest";
import {
  buildReportQuery,
  buildRequestsQuery,
  normalizeReportRows,
  normalizeRequestRow,
  reportRowsToCsv,
  sortReportRows,
  sumReportRows,
  toCsv,
} from "../gateway-analytics";

describe("toCsv", () => {
  it("writes a header row and quotes commas, quotes and newlines", () => {
    const csv = toCsv(["a", "b"], [["x,y", 'say "hi"'], ["line\nbreak", 3]]);
    expect(csv).toBe('a,b\n"x,y","say ""hi"""\n"line\nbreak",3');
  });
});

describe("report helpers", () => {
  const rows = normalizeReportRows({
    rows: [
      { group: "2026-11-02", requests: "2", prompt_tokens: 10, completion_tokens: 5, cost_usd: 0.5 },
      { group: "2026-11-01", requests: 1, prompt_tokens: 4, completion_tokens: 1, cost_usd: 0.25 },
      { group: "bad" },
    ],
  });

  it("coerces numbers and tolerates a missing rows array", () => {
    expect(rows[0].requests).toBe(2);
    expect(rows[2]).toMatchObject({ requests: 0, cost_usd: 0 });
    expect(normalizeReportRows(null)).toEqual([]);
  });

  it("sums rows", () => {
    expect(sumReportRows(rows)).toEqual({
      requests: 3,
      prompt_tokens: 14,
      completion_tokens: 6,
      cost_usd: 0.75,
    });
  });

  it("sorts days ascending and other groups by cost", () => {
    expect(sortReportRows(rows, "day").map((r) => r.group)).toEqual(["2026-11-01", "2026-11-02", "bad"]);
    expect(sortReportRows(rows, "model").map((r) => r.group)).toEqual(["2026-11-02", "2026-11-01", "bad"]);
  });

  it("renders CSV with a header row", () => {
    const csv = reportRowsToCsv("Model", rows.slice(0, 1));
    expect(csv.split("\n")[0]).toBe("Model,Requests,Tokens in,Tokens out,Cost (USD)");
    expect(csv.split("\n")[1]).toBe("2026-11-02,2,10,5,0.500000");
  });

  it("builds the report query from the contract", () => {
    const q = new URLSearchParams(
      buildReportQuery(
        { from: "2026-11-01", to: "2026-11-13", groupBy: "project", projectId: "p1", client: "gizzi-code", scope: "org" },
        "csv",
      ),
    );
    expect(Object.fromEntries(q)).toEqual({
      from: "2026-11-01",
      to: "2026-11-13",
      group_by: "project",
      project_id: "p1",
      client: "gizzi-code",
      scope: "org",
      format: "csv",
    });
    const plain = new URLSearchParams(buildReportQuery({ from: "a", to: "b", groupBy: "day", projectId: "" }));
    expect(plain.has("project_id")).toBe(false);
    expect(plain.has("scope")).toBe(false);
    expect(plain.has("format")).toBe(false);
  });
});

describe("request log helpers", () => {
  it("builds the list query and skips empty filters", () => {
    expect(buildRequestsQuery({ projectId: "", cursor: "c1", limit: 50 })).toBe("cursor=c1&limit=50");
  });

  it("adds scope=org only for org scope", () => {
    expect(buildRequestsQuery({ scope: "org", limit: 10 })).toBe("limit=10&scope=org");
    expect(buildRequestsQuery({ scope: "me", limit: 10 })).toBe("limit=10");
  });

  it("normalizes a row", () => {
    expect(
      normalizeRequestRow({ id: "req_1", created_at: "t", model_id: "m", prompt_tokens: 3, cost_usd: "0.1", latency_ms: 12 }),
    ).toMatchObject({ request_id: "req_1", model: "m", prompt_tokens: 3, cost_usd: 0.1, latency_ms: 12, client: null });
  });
});
