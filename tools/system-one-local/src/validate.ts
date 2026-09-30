// Request validation for POST /v1/systemone. Mirrors the documented limits:
// choice ≤ 255 options, score 2–10 levels, noul criteria only {true,false}.
import { SystemOneError, type SystemOneRequest } from "./types.ts";

export const MAX_CHOICE_OPTIONS = 255;
export const MIN_SCORE_LEVELS = 2;
export const MAX_SCORE_LEVELS = 10;
export const MAX_QUESTIONS = 64;

type Issue = { path: string; message: string };

const isObj = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);
const isStructured = (v: unknown) =>
  typeof v === "string" || Array.isArray(v) || isObj(v);

export function validateRequest(body: unknown): SystemOneRequest {
  const issues: Issue[] = [];
  if (!isObj(body)) {
    throw invalid([{ path: "", message: "body must be a JSON object" }]);
  }
  if (typeof body.model !== "string" || body.model.length === 0) {
    issues.push({ path: "model", message: "required non-empty string" });
  }
  if (!("state" in body) || !isStructured(body.state)) {
    issues.push({ path: "state", message: "required: string, object, or array" });
  } else if (typeof body.state === "string" && body.state.trim() === "") {
    issues.push({ path: "state", message: "must not be empty" });
  }
  const qs = body.questions;
  if (!isObj(qs) || Object.keys(qs).length === 0) {
    issues.push({ path: "questions", message: "required non-empty map of id → Question" });
  } else {
    const ids = Object.keys(qs);
    if (ids.length > MAX_QUESTIONS) {
      issues.push({ path: "questions", message: `at most ${MAX_QUESTIONS} questions per request` });
    }
    for (const id of ids) validateQuestion(`questions.${id}`, qs[id], issues);
  }
  if (issues.length) throw invalid(issues);
  return body as unknown as SystemOneRequest;
}

function validateQuestion(path: string, q: unknown, issues: Issue[]) {
  if (!isObj(q)) {
    issues.push({ path, message: "must be an object" });
    return;
  }
  if (!isStructured(q.instructions)) {
    issues.push({ path: `${path}.instructions`, message: "required: string, object, or array" });
  }
  switch (q.type) {
    case "noul": {
      if (q.criteria === undefined) break;
      if (!isObj(q.criteria)) {
        issues.push({ path: `${path}.criteria`, message: "noul criteria must be an object with optional `true`/`false`" });
        break;
      }
      for (const k of Object.keys(q.criteria)) {
        if (k !== "true" && k !== "false") {
          issues.push({ path: `${path}.criteria.${k}`, message: "noul criteria only accepts `true` and `false`" });
        } else if (!isStructured(q.criteria[k])) {
          issues.push({ path: `${path}.criteria.${k}`, message: "must be string, object, or array" });
        }
      }
      break;
    }
    case "choice": {
      if (!isObj(q.criteria)) {
        issues.push({ path: `${path}.criteria`, message: "choice criteria is a required map option → description|null" });
        break;
      }
      const opts = Object.keys(q.criteria);
      if (opts.length < 2) issues.push({ path: `${path}.criteria`, message: "choice needs at least 2 options" });
      if (opts.length > MAX_CHOICE_OPTIONS) {
        issues.push({ path: `${path}.criteria`, message: `choice accepts at most ${MAX_CHOICE_OPTIONS} options` });
      }
      for (const o of opts) {
        const v = q.criteria[o];
        if (o.trim() === "") issues.push({ path: `${path}.criteria`, message: "option keys must be non-empty" });
        if (v !== null && !isStructured(v)) {
          issues.push({ path: `${path}.criteria.${o}`, message: "must be string, object, array, or null" });
        }
      }
      break;
    }
    case "score": {
      if (!Array.isArray(q.criteria)) {
        issues.push({ path: `${path}.criteria`, message: "score criteria is a required ordered array of levels" });
        break;
      }
      const n = q.criteria.length;
      if (n < MIN_SCORE_LEVELS || n > MAX_SCORE_LEVELS) {
        issues.push({ path: `${path}.criteria`, message: `score needs ${MIN_SCORE_LEVELS}–${MAX_SCORE_LEVELS} levels (got ${n})` });
      }
      q.criteria.forEach((lvl, i) => {
        if (!isStructured(lvl)) issues.push({ path: `${path}.criteria[${i}]`, message: "must be string, object, or array" });
      });
      break;
    }
    default:
      issues.push({ path: `${path}.type`, message: 'must be "noul", "choice", or "score"' });
  }
}

function invalid(details: Issue[]) {
  return new SystemOneError(422, {
    error: {
      type: "invalid_request_error",
      message: `request failed validation: ${details.map((d) => `${d.path || "body"}: ${d.message}`).join("; ")}`,
      details,
    },
  });
}
