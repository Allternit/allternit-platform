// §A6.6 — artifact store: content-addressed commit, quarantine xattr, sha256 +
// magic-byte MIME verify, relative local_path, never auto-opens.
import { createHash } from "node:crypto";
import { existsSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { ArtifactFile, ProviderArtifactRef } from "@allternit/subscription-fabric-contracts";
import { createArtifactStore, quarantineFor, type ArtifactSourceContext } from "../src/artifacts/store.js";
import { openDatabase, type Db } from "../src/store/db.js";
import { getArtifact } from "../src/store/queries.js";
import { cleanupDir, tmpStateDir } from "./helpers.js";

const PNG = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 1, 2, 3, 4]);
const ZIP = new Uint8Array([0x50, 0x4b, 0x03, 0x04, 9, 9, 9, 9]);

function sha256(b: Uint8Array): string {
  return createHash("sha256").update(b).digest("hex");
}

function fileOf(bytes: Uint8Array, format: string, mime: string, sha?: string): ArtifactFile {
  return {
    data: bytes,
    mime_type: mime,
    format,
    sha256: sha ?? sha256(bytes),
    size_bytes: bytes.length,
  };
}

const REF: ProviderArtifactRef = {
  provider: "fixture-web" as ProviderArtifactRef["provider"],
  provider_artifact_id: "fw-img-1",
  provider_url: "https://fixture-web.test/a/fw-img-1",
  provider_url_expires_at: null,
};

function source(taskId = "task-art-1"): ArtifactSourceContext {
  return {
    task_id: taskId,
    attempt_no: 1,
    capability: "chat.create",
    provider: "fixture-web" as ArtifactSourceContext["provider"],
    account_id: "acct-fw-1",
    adapter_id: "fixture-web",
    adapter_version: "0.1.0",
    thread_id: null,
    project_id: null,
    bot_id: null,
    sensitivity: "internal",
  };
}

describe("artifacts/store", () => {
  let dir: string;
  let db: Db;

  beforeEach(() => {
    dir = tmpStateDir();
    db = openDatabase(":memory:");
  });
  afterEach(() => {
    db.close();
    cleanupDir(dir);
  });

  it("commit lands bytes at the sharded sha path with quarantine xattr and a relative local_path", async () => {
    const sink = createArtifactStore(db, { artifactsDir: join(dir, "artifacts"), source: source() });
    const id = await sink.begin(REF, { mime_type: "image/png", format: "png", title: "img" });
    await sink.write(id, PNG.subarray(0, 5));
    await sink.write(id, PNG.subarray(5));
    await sink.commit(id, fileOf(PNG, "png", "image/png"));

    const sha = sha256(PNG);
    const abs = join(dir, "artifacts", sha.slice(0, 2), sha);
    expect(existsSync(abs)).toBe(true);
    expect(new Uint8Array(readFileSync(abs))).toEqual(PNG);

    const row = getArtifact(db, id);
    expect(row?.storage.retrieval_state).toBe("local");
    expect(row?.storage.local_path).toBe(`${sha.slice(0, 2)}/${sha}`); // relative, never absolute
    expect(row?.storage.sha256).toBe(sha);
    expect(row?.storage.size_bytes).toBe(PNG.length);
    expect(row?.source.provider_artifact_id).toBe("fw-img-1");

    const read = spawnSync("/usr/bin/xattr", ["-p", "com.apple.quarantine", abs]);
    expect(read.status).toBe(0);
    expect(read.stdout.toString().trim().length).toBeGreaterThan(0);
    // tmp file is gone
    expect(existsSync(join(dir, "artifacts", "tmp", id))).toBe(false);
  });

  it("sha256 mismatch → retrieval_state failed, no content-addressed file", async () => {
    const sink = createArtifactStore(db, { artifactsDir: join(dir, "artifacts"), source: source() });
    const id = await sink.begin(REF, { format: "png" });
    await sink.write(id, PNG);
    await expect(
      sink.commit(id, fileOf(PNG, "png", "image/png", "0".repeat(64)))
    ).rejects.toThrow(/sha256 mismatch/);
    expect(getArtifact(db, id)?.storage.retrieval_state).toBe("failed");
    expect(existsSync(join(dir, "artifacts", "tmp", id))).toBe(false);
  });

  it("MIME mismatch (declared pdf, magic bytes png) → failed", async () => {
    const sink = createArtifactStore(db, { artifactsDir: join(dir, "artifacts"), source: source() });
    const id = await sink.begin(REF, { format: "pdf" });
    await sink.write(id, PNG);
    await expect(sink.commit(id, fileOf(PNG, "pdf", "application/pdf"))).rejects.toThrow(
      /mime mismatch/
    );
    expect(getArtifact(db, id)?.storage.retrieval_state).toBe("failed");
  });

  it("unrecognized magic bytes → failed", async () => {
    const sink = createArtifactStore(db, { artifactsDir: join(dir, "artifacts"), source: source() });
    const id = await sink.begin(REF, {});
    await sink.write(id, new Uint8Array([0x00, 0x11, 0x22, 0x33]));
    await expect(
      sink.commit(id, fileOf(new Uint8Array([0x00, 0x11, 0x22, 0x33]), "bin", "application/octet-stream"))
    ).rejects.toThrow(/unrecognized content/);
    expect(getArtifact(db, id)?.storage.retrieval_state).toBe("failed");
  });

  it("OOXML containers sniff as zip and still verify as their declared format", async () => {
    const sink = createArtifactStore(db, { artifactsDir: join(dir, "artifacts"), source: source() });
    const id = await sink.begin(REF, { format: "pptx" });
    await sink.write(id, ZIP);
    await sink.commit(
      id,
      fileOf(ZIP, "pptx", "application/vnd.openxmlformats-officedocument.presentationml.presentation")
    );
    const row = getArtifact(db, id);
    expect(row?.storage.retrieval_state).toBe("local");
    expect(row?.format).toBe("pptx");
    expect(row?.type).toBe("presentation");
  });

  it("fail() marks the row failed and removes the temp file", async () => {
    const sink = createArtifactStore(db, { artifactsDir: join(dir, "artifacts"), source: source() });
    const id = await sink.begin(REF, { format: "png" });
    await sink.write(id, PNG.subarray(0, 4));
    await sink.fail(id, "download aborted");
    expect(getArtifact(db, id)?.storage.retrieval_state).toBe("failed");
    expect(existsSync(join(dir, "artifacts", "tmp", id))).toBe(false);
    // idempotent — the SDK calls fail() again after a commit throws
    await sink.fail(id, "again");
    expect(getArtifact(db, id)?.storage.retrieval_state).toBe("failed");
  });
});

describe("quarantine on Linux Sessions machines (§A6)", () => {
  it("makes the artifact read-only with no exec bit (mandatory) — never throws for a missing xattr tool", () => {
    const d = tmpStateDir();
    const f = join(d, "artifact.bin");
    writeFileSync(f, "x", { mode: 0o755 });
    quarantineFor("linux")(f);
    expect(statSync(f).mode & 0o777).toBe(0o444);
    cleanupDir(d);
  });
});
