import { existsSync, utimesSync } from "node:fs";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { Page } from "playwright";
import { captureEvidence, pruneEvidence, setEvidenceSink } from "../adapters/_shared/evidence.js";
import { getEvidence, insertEvidence, latestEvidenceId } from "../src/http/routes_evidence.js";
import { openDatabase, type Db } from "../src/store/db.js";
import { cleanupDir, tmpStateDir } from "./helpers.js";

const fakePage = {
  screenshot: async () => Buffer.from("png"),
  evaluate: async () => '<div>Signed in as jane.doe@example.com <input value="x"></div>',
  url: () => "https://claude.ai/new?token=secret#frag",
} as unknown as Page;

describe("ui_bridge failure evidence", () => {
  let dir: string;
  let db: Db;
  beforeEach(() => {
    dir = tmpStateDir();
    process.env.ALLTERNIT_EVIDENCE_DIR = dir;
    db = openDatabase(":memory:");
    setEvidenceSink((row) => insertEvidence(db, row));
  });
  afterEach(() => {
    setEvidenceSink(null);
    delete process.env.ALLTERNIT_EVIDENCE_DIR;
    cleanupDir(dir);
  });

  it("saves a row for a selector_not_found with the key, a clean URL and a redacted DOM excerpt", async () => {
    const id = await captureEvidence(fakePage, { taskId: "t1", adapterId: "claude-web", selectorKey: "composer", detail: 'selector key "composer" matched nothing' });
    expect(id).toBeTruthy();
    const row = getEvidence(db, id!)!;
    expect(row.selector_key).toBe("composer");
    expect(row.url).toBe("https://claude.ai/new");
    expect(row.dom_excerpt).toContain("[redacted:email]");
    expect(row.dom_excerpt).not.toContain("jane.doe@example.com");
    expect(existsSync(join(dir, row.screenshot_file!))).toBe(true);
    expect(latestEvidenceId(db, "claude-web")).toBe(id);
  });

  it("drops evidence quietly without a sink, and prunes screenshots older than 7 days", async () => {
    const id = await captureEvidence(fakePage, { selectorKey: "composer" });
    const old = join(dir, `${id}.png`);
    const eightDays = (Date.now() - 8 * 24 * 3600 * 1000) / 1000;
    utimesSync(old, eightDays, eightDays);
    expect(pruneEvidence(dir)).toBe(1);
    setEvidenceSink(null);
    expect(await captureEvidence(fakePage, {})).toBeNull();
  });
});
