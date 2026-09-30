#!/usr/bin/env bun
// system-one CLI.
//
//   system-one serve [--port 7717]
//   system-one ask --state <file|-> --questions <file> [--model jev-latest] [--server http://127.0.0.1:7717]
//   system-one models
//   system-one route-model --task "..." --allow-paid       (paid OpenRouter call)
//
// `ask` evaluates in-process unless --server is given. A state file ending in
// .json is parsed as JSON (object/array/string); anything else is sent as text.
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { calibrateAndWrite } from "./decision/calibrate.ts";
import { harvest, readDataset, ShadowLedger, writeDataset } from "./decision/shadow.ts";
import { BASE_DIR } from "./log.ts";
import { SystemOne } from "./engine.ts";
import { routeModel } from "./route-model.ts";
import { DEFAULT_PORT, HOST, serve } from "./server.ts";
import { SystemOneError } from "./types.ts";

function flags(argv: string[]) {
  const out: Record<string, string | true> = {};
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (!a.startsWith("--")) continue;
    const k = a.slice(2);
    const v = argv[i + 1];
    if (v === undefined || v.startsWith("--")) out[k] = true;
    else { out[k] = v; i++; }
  }
  return out;
}

function readState(path: string) {
  const raw = path === "-" ? readFileSync(0, "utf8") : readFileSync(path, "utf8");
  return path.endsWith(".json") ? JSON.parse(raw) : raw;
}

async function main() {
  const [cmd, ...rest] = process.argv.slice(2);
  const f = flags(rest);
  switch (cmd) {
    case "serve": {
      const s = serve({ port: f.port ? Number(f.port) : undefined });
      console.error(`system-one listening on http://${HOST}:${s.port} (runtime model: ${new SystemOne().local.model})`);
      return;
    }
    case "ask": {
      if (typeof f.state !== "string" || typeof f.questions !== "string") {
        throw new Error("usage: system-one ask --state <file|-> --questions <file> [--model m] [--server url]");
      }
      const qfile = JSON.parse(readFileSync(f.questions, "utf8"));
      const body = {
        model: typeof f.model === "string" ? f.model : "jev-latest",
        state: readState(f.state),
        questions: qfile.questions ?? qfile,
      };
      if (typeof f.server === "string") {
        const res = await fetch(`${f.server.replace(/\/$/, "")}/v1/systemone`, {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify(body),
        });
        console.log(JSON.stringify(await res.json(), null, 2));
        if (!res.ok) process.exitCode = 1;
      } else {
        console.log(JSON.stringify(await new SystemOne().evaluate(body), null, 2));
      }
      return;
    }
    case "models":
      console.log(JSON.stringify(new SystemOne().models(), null, 2));
      return;
    case "route-model": {
      if (typeof f.task !== "string") throw new Error('usage: system-one route-model --task "..." --allow-paid');
      console.log(JSON.stringify(await routeModel({ task: f.task, allowPaid: f["allow-paid"] === true }), null, 2));
      return;
    }
    case "harvest": {
      const dir = typeof f["shadow-dir"] === "string" ? f["shadow-dir"] : (process.env.ALLTERNIT_S1_SHADOW_DIR ?? join(BASE_DIR, "shadow"));
      if (typeof f.out !== "string") throw new Error("usage: system-one harvest --out <jsonl> [--shadow-dir d] [--primitive id] [--model ref]");
      const { rows, stats } = harvest(dir, { primitive: typeof f.primitive === "string" ? f.primitive : undefined, model: typeof f.model === "string" ? f.model : undefined });
      writeDataset(rows, f.out);
      console.log(JSON.stringify({ out: f.out, ...stats }, null, 2));
      return;
    }
    case "outcome": {
      const dir = typeof f["shadow-dir"] === "string" ? f["shadow-dir"] : (process.env.ALLTERNIT_S1_SHADOW_DIR ?? join(BASE_DIR, "shadow"));
      if (typeof f.truth !== "string" || typeof f.source !== "string") throw new Error("usage: system-one outcome --truth <candidate_id> --source <verifier:x> (--decision-id id | --subject-ref ref [--question-id q])");
      const str = (k: string) => (typeof f[k] === "string" ? (f[k] as string) : null);
      console.log(JSON.stringify(new ShadowLedger(dir).recordOutcome({ truth: f.truth, source: f.source, decision_id: str("decision-id"), subject_ref: str("subject-ref"), question_id: str("question-id") })));
      return;
    }
    case "calibrate": {
      if (typeof f.data !== "string" || typeof f.primitive !== "string" || typeof f.model !== "string") {
        throw new Error("usage: system-one calibrate --data <jsonl> --primitive <id> --model <ref> [--manifests path] [--report path] [--holdout 0.4] [--strict]");
      }
      const rep = calibrateAndWrite(readDataset(f.data), {
        primitive: f.primitive, model: f.model, datasetRef: f.data, strict: f.strict === true,
        holdout: typeof f.holdout === "string" ? Number(f.holdout) : undefined,
        manifestsPath: typeof f.manifests === "string" ? f.manifests : process.env.ALLTERNIT_S1_MANIFESTS?.trim() || undefined,
        reportPath: typeof f.report === "string" ? f.report : `${f.data}.calibration-report.json`,
      });
      console.log(JSON.stringify({ passed: rep.passed, manifests_written: rep.manifests_written, scopes: rep.scopes.map((s: any) => ({ n_total: s.n_total, held_out: s.n_held_out, gate_passed: s.gate_passed, failures: s.failures, needs: s.needs })), notes: rep.notes }, null, 2));
      process.exitCode = rep.passed ? 0 : 3;
      return;
    }
    default:
      console.error(`usage: system-one <serve|ask|models|route-model|harvest|outcome|calibrate> (default port ${DEFAULT_PORT})`);
      break;
  }
  if (!["serve", "ask", "models", "route-model", "harvest", "outcome", "calibrate"].includes(cmd ?? "")) process.exitCode = 2;
}

main().catch((e) => {
  if (e instanceof SystemOneError) console.error(JSON.stringify(e.body, null, 2));
  else console.error(`error: ${(e as Error).message}`);
  process.exitCode = 1;
});
