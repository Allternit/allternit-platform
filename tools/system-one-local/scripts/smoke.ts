#!/usr/bin/env bun
// Real local smoke: POSTs the example 3-question pack (noul + choice + score) to a
// running server and prints the response plus wall-clock latency.
//   bun src/cli.ts serve &   then   bun scripts/smoke.ts [--url http://127.0.0.1:7717] [--runs 3]
import { readFileSync } from "node:fs";
import { join } from "node:path";

const args = process.argv.slice(2);
const url = args.includes("--url") ? args[args.indexOf("--url") + 1] : "http://127.0.0.1:7717";
const runs = args.includes("--runs") ? Number(args[args.indexOf("--runs") + 1]) : 3;
const dir = join(import.meta.dir, "..", "examples");
const body = {
  model: "jev-latest",
  state: readFileSync(join(dir, "state.txt"), "utf8").trim(),
  questions: JSON.parse(readFileSync(join(dir, "questions.json"), "utf8")),
};
const times: number[] = [];
let last: unknown;
for (let i = 0; i < runs; i++) {
  const t0 = performance.now();
  const res = await fetch(`${url}/v1/systemone`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });
  last = await res.json();
  times.push(Math.round(performance.now() - t0));
  if (!res.ok) {
    console.error(JSON.stringify(last, null, 2));
    process.exit(1);
  }
}
console.log(JSON.stringify(last, null, 2));
console.log(`latency ms per run (3 questions): ${times.join(", ")}  (first run may include model load)`);
