// §A6.6 — the real contracts ArtifactSink: temp-file streaming, sha256 verify,
// magic-byte MIME verify, content-addressed move, quarantine (macOS
// com.apple.quarantine; Linux read-only/no-exec + user xattr).
// Never auto-opens anything; local_path stays relative to the artifact root.
import { spawnSync } from "node:child_process";
import { createHash, randomUUID, type Hash } from "node:crypto";
import {
  chmodSync,
  closeSync,
  mkdirSync,
  openSync,
  readSync,
  renameSync,
  rmSync,
  writeSync,
} from "node:fs";
import { dirname, join } from "node:path";
import { sniffMime } from "@allternit/subscription-adapter-sdk";
import type {
  Artifact,
  ArtifactFile,
  ArtifactSink,
  ArtifactType,
  CapabilityId,
  ProviderArtifactRef,
  ProviderId,
  Sensitivity,
} from "@allternit/subscription-fabric-contracts";
import type { Db } from "../store/db.js";
import { getArtifact, insertArtifact, updateArtifactStorage } from "../store/queries.js";

export interface ArtifactSourceContext {
  task_id: string;
  attempt_no: number;
  capability: CapabilityId;
  provider: ProviderId;
  account_id: string;
  adapter_id: string;
  adapter_version: string;
  thread_id: string | null;
  project_id: string | null;
  bot_id: string | null;
  sensitivity: Sensitivity;
}

export interface ArtifactStoreConfig {
  artifactsDir: string;
  source: ArtifactSourceContext;
  // Injectable for tests/non-macOS; default shells out to /usr/bin/xattr -w.
  setQuarantine?: (path: string) => void;
}

interface PendingArtifact {
  tmpPath: string;
  fd: number;
  hasher: Hash;
  size: number;
}

// OOXML containers sniff as zip; a declared office format is a match.
const OOXML_FORMATS = new Set(["pptx", "docx", "xlsx"]);

const FORMAT_TO_TYPE: Record<string, ArtifactType> = {
  png: "image",
  jpeg: "image",
  gif: "image",
  webp: "image",
  pdf: "pdf",
  pptx: "presentation",
  docx: "document",
  xlsx: "spreadsheet",
  zip: "archive",
};

// §A6 quarantine: provider output is untrusted — never executable, never
// auto-opened. macOS: com.apple.quarantine (Gatekeeper). Linux (Sessions
// machines): read-only, no exec bit (mandatory) + a best-effort
// user.allternit.quarantine xattr where the filesystem supports it.
export function quarantineFor(platform: NodeJS.Platform): (path: string) => void {
  return (path: string): void => {
    const stamp = Math.floor(Date.now() / 1000).toString(16);
    const value = `0081;${stamp};subscription-gateway;`;
    if (platform === "darwin") {
      const res = spawnSync("/usr/bin/xattr", ["-w", "com.apple.quarantine", value, path], {
        stdio: "ignore",
      });
      if (res.status !== 0) {
        throw new Error(
          `xattr quarantine failed for ${path}: ${res.error?.message ?? `exit ${res.status}`}`
        );
      }
      return;
    }
    chmodSync(path, 0o444);
    spawnSync("setfattr", ["-n", "user.allternit.quarantine", "-v", value, path], { stdio: "ignore" });
  };
}

const defaultSetQuarantine = quarantineFor(process.platform);

export function createArtifactStore(db: Db, config: ArtifactStoreConfig): ArtifactSink {
  const setQuarantine = config.setQuarantine ?? defaultSetQuarantine;
  const tmpDir = join(config.artifactsDir, "tmp");
  const pending = new Map<string, PendingArtifact>();

  return {
    async begin(ref, meta = {}) {
      const artifactId = randomUUID();
      const now = new Date().toISOString();
      const artifact: Artifact = {
        artifact_id: artifactId,
        type: (meta.format && FORMAT_TO_TYPE[meta.format]) || "document",
        mime_type: meta.mime_type ?? null,
        format: meta.format ?? null,
        title: meta.title ?? null,
        source: {
          task_id: config.source.task_id,
          attempt_no: config.source.attempt_no,
          capability: config.source.capability,
          provider: config.source.provider,
          account_id: config.source.account_id,
          adapter_id: config.source.adapter_id,
          adapter_version: config.source.adapter_version,
          provider_artifact_id: ref.provider_artifact_id,
          provider_url: ref.provider_url,
          provider_url_expires_at: ref.provider_url_expires_at,
        },
        context: {
          thread_id: config.source.thread_id,
          project_id: config.source.project_id,
          bot_id: config.source.bot_id,
        },
        storage: {
          retrieval_state: "downloading",
          local_path: null,
          sha256: null,
          size_bytes: null,
          local_preview_path: null,
        },
        lineage: { version: 1, parent_artifact_id: null },
        capabilities: { editable_via: [], export_formats: [] },
        trust: "untrusted_provider_output",
        sensitivity: config.source.sensitivity,
        created_at: now,
      };
      insertArtifact(db, artifact);
      mkdirSync(tmpDir, { recursive: true });
      const tmpPath = join(tmpDir, artifactId);
      pending.set(artifactId, {
        tmpPath,
        fd: openSync(tmpPath, "w"),
        hasher: createHash("sha256"),
        size: 0,
      });
      return artifactId;
    },

    async write(artifactId, chunk) {
      const p = pending.get(artifactId);
      if (!p) throw new Error(`artifact ${artifactId} is not open for writing`);
      writeSync(p.fd, chunk);
      p.hasher.update(chunk);
      p.size += chunk.length;
    },

    async commit(artifactId, file) {
      const p = pending.get(artifactId);
      if (!p) throw new Error(`artifact ${artifactId} is not open for commit`);
      pending.delete(artifactId);
      closeSync(p.fd);

      const failCommit = async (reason: string): Promise<never> => {
        await this.fail(artifactId, reason);
        throw new Error(`artifact ${artifactId} commit failed: ${reason}`);
      };

      const sha256 = p.hasher.digest("hex");
      if (sha256 !== file.sha256) {
        return failCommit(`sha256 mismatch (expected ${file.sha256}, got ${sha256})`);
      }

      const head = new Uint8Array(16);
      const fd = openSync(p.tmpPath, "r");
      const read = readSync(fd, head, 0, 16, 0);
      closeSync(fd);
      const sniffed = sniffMime(head.subarray(0, read));
      if (!sniffed) {
        return failCommit("unrecognized content (no known magic bytes)");
      }
      const declared = file.format;
      const ooxmlOk = OOXML_FORMATS.has(declared) && sniffed.format === "zip";
      if (declared && sniffed.format !== declared && !ooxmlOk) {
        return failCommit(
          `mime mismatch (declared ${declared}, magic bytes say ${sniffed.format})`
        );
      }

      const relPath = `${sha256.slice(0, 2)}/${sha256}`;
      const target = join(config.artifactsDir, relPath);
      mkdirSync(dirname(target), { recursive: true });
      renameSync(p.tmpPath, target);
      setQuarantine(target);

      updateArtifactStorage(db, artifactId, {
        retrieval_state: "local",
        local_path: relPath,
        sha256,
        size_bytes: p.size,
        mime_type: OOXML_FORMATS.has(declared) ? file.mime_type : sniffed.mime_type,
        format: declared || sniffed.format,
      });
    },

    async fail(artifactId, _reason) {
      const p = pending.get(artifactId);
      if (p) {
        pending.delete(artifactId);
        try {
          closeSync(p.fd);
        } catch {
          // already closed
        }
        rmSync(p.tmpPath, { force: true });
      } else {
        rmSync(join(tmpDir, artifactId), { force: true });
      }
      const row = getArtifact(db, artifactId);
      if (row && row.storage.retrieval_state !== "local") {
        updateArtifactStorage(db, artifactId, { retrieval_state: "failed" });
      }
    },
  };
}
