import { describe, expect, test } from "bun:test";
import { validateRequest } from "../src/validate.ts";
import { SystemOneError } from "../src/types.ts";

const ok = {
  model: "jev-latest",
  state: "Help! My payouts have been failing for 3 days.",
  questions: {
    is_urgent: { type: "noul", instructions: "Does this convey urgency?", criteria: { true: "time-sensitive", false: "no" } },
    department: { type: "choice", instructions: "Which team?", criteria: { billing: "Payments", technical: null } },
    frustration: { type: "score", instructions: "How frustrated?", criteria: ["Calm", "Frustrated", "Very angry"] },
  },
};

function fails(body: unknown, path: string) {
  try {
    validateRequest(body);
  } catch (e) {
    expect(e).toBeInstanceOf(SystemOneError);
    const err = e as SystemOneError;
    expect(err.status).toBe(422);
    expect(err.body.error.type).toBe("invalid_request_error");
    expect(err.body.error.details?.some((d) => d.path.startsWith(path))).toBe(true);
    return;
  }
  throw new Error(`expected 422 at ${path}`);
}

describe("validateRequest", () => {
  test("accepts the documented shapes (string, object, array state)", () => {
    expect(validateRequest(ok)).toBeTruthy();
    expect(validateRequest({ ...ok, state: { ticket: { text: "x" } } })).toBeTruthy();
    expect(validateRequest({ ...ok, state: ["a", "b"] })).toBeTruthy();
    expect(validateRequest({ ...ok, questions: { q: { type: "noul", instructions: { question: "x?", data: 1 } } } })).toBeTruthy();
  });
  test("missing model", () => fails({ ...ok, model: undefined }, "model"));
  test("number state", () => fails({ ...ok, state: 5 }, "state"));
  test("empty questions", () => fails({ ...ok, questions: {} }, "questions"));
  test("bad type", () => fails({ ...ok, questions: { q: { type: "bool", instructions: "x" } } }, "questions.q.type"));
  test("missing instructions", () => fails({ ...ok, questions: { q: { type: "noul" } } }, "questions.q.instructions"));
  test("noul criteria only true/false", () =>
    fails({ ...ok, questions: { q: { type: "noul", instructions: "x", criteria: { maybe: "?" } } } }, "questions.q.criteria.maybe"));
  test("choice criteria required", () => fails({ ...ok, questions: { q: { type: "choice", instructions: "x" } } }, "questions.q.criteria"));
  test("choice > 255 options", () => {
    const criteria = Object.fromEntries(Array.from({ length: 256 }, (_, i) => [`o${i}`, null]));
    fails({ ...ok, questions: { q: { type: "choice", instructions: "x", criteria } } }, "questions.q.criteria");
  });
  test("choice with 255 options is fine", () => {
    const criteria = Object.fromEntries(Array.from({ length: 255 }, (_, i) => [`o${i}`, null]));
    expect(validateRequest({ ...ok, questions: { q: { type: "choice", instructions: "x", criteria } } })).toBeTruthy();
  });
  test("score needs 2–10 levels", () => {
    fails({ ...ok, questions: { q: { type: "score", instructions: "x", criteria: ["one"] } } }, "questions.q.criteria");
    fails({ ...ok, questions: { q: { type: "score", instructions: "x", criteria: Array(11).fill("l") } } }, "questions.q.criteria");
  });
  test("score criteria must be an array", () =>
    fails({ ...ok, questions: { q: { type: "score", instructions: "x", criteria: { a: 1 } } } }, "questions.q.criteria"));
});
