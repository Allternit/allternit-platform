// Optional local call log. OFF by default (SYSTEM_ONE_LOG=1 to enable).
// Records a hash of the request, token counts and answers — never the state
// or question text.
import { appendFileSync, mkdirSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import { createHash } from "node:crypto";
import type { SystemOneRequest, SystemOneResponse } from "./types.ts";

export const BASE_DIR = join(homedir(), ".allternit", "system-one");

export function sha256(s: string) {
  return createHash("sha256").update(s).digest("hex");
}

export function today() {
  return new Date().toISOString().slice(0, 10);
}

export function appendJsonl(dir: string, record: unknown) {
  mkdirSync(dir, { recursive: true, mode: 0o700 });
  appendFileSync(join(dir, `${today()}.jsonl`), `${JSON.stringify(record)}\n`, { mode: 0o600 });
}

export function logCall(req: SystemOneRequest, res: SystemOneResponse, enabled = process.env.SYSTEM_ONE_LOG === "1") {
  if (!enabled) return;
  try {
    appendJsonl(process.env.SYSTEM_ONE_LOG_DIR ?? join(BASE_DIR, "log"), {
      ts: new Date().toISOString(),
      request_sha256: sha256(JSON.stringify({ state: req.state, questions: req.questions })),
      model: res.model,
      question_ids: Object.keys(req.questions),
      usage: res.usage,
      answers: res.answers,
      x_allternit: res.x_allternit,
    });
  } catch {
    // Logging must never break a call.
  }
}
