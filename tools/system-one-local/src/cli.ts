#!/usr/bin/env bun
// system-one CLI.
//
//   system-one serve [--port 7717]
//   system-one ask --state <file|-> --questions <file> [--model jev-latest] [--server http://127.0.0.1:7717]
//   system-one models
//   system-one route-model --task "..." --allow-paid       (paid OpenRouter call)
//   system-one export --out <dir>                            ledger -> fine-tuning set (WP-L1)
//   system-one calibrate --tune <jsonl> --cert <jsonl>       Q26 gate (legacy Q22: --data ... --primitive ... --model ...)
//   system-one canary <status|enable|grow|sync|rollback>     Q26 exposure budget + CUSUM rollback
//
// `ask` evaluates in-process unless --server is given. A state file ending in
// .json is parsed as JSON (object/array/string); anything else is sent as text.
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { appendManifests, calibrateAndWrite } from "./decision/calibrate.ts";
import { CanaryController } from "./decision/canary.ts";
import { buildExport, writeExport } from "./decision/export.ts";
import { runQ26, type Q26Policy, type Q26Report } from "./decision/q26.ts";
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
      // Q28: a running server keeps the shadow ledger by default (hashes, options,
      // distributions, outcome labels; never raw state text). Opt out with
      // SYSTEM_ONE_SHADOW_LOG=0. Library/test use stays off (see shadowLedger()).
      if (!process.env.ALLTERNIT_S1_SHADOW_DIR?.trim() && process.env.SYSTEM_ONE_SHADOW_LOG !== "0") {
        process.env.ALLTERNIT_S1_SHADOW_DIR = join(BASE_DIR, "shadow");
      }
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
    case "hook-outcome": {
      // Harness outcome hook (PermissionRequest / PostToolUse / PostToolUseFailure / Stop),
      // same as hooks/s1-outcome but runnable from the compiled binary. Never prints a
      // decision, always exits 0. CommRails registers it in its session settings by default.
      try {
        const { handleOutcomeHook } = await import("./hook/guard.ts");
        await handleOutcomeHook(JSON.parse(readFileSync(0, "utf8")));
      } catch {
        // outcome labels are best-effort
      }
      return;
    }
    case "outcome": {
      const dir = typeof f["shadow-dir"] === "string" ? f["shadow-dir"] : (process.env.ALLTERNIT_S1_SHADOW_DIR ?? join(BASE_DIR, "shadow"));
      if (typeof f.truth !== "string" || typeof f.source !== "string") throw new Error("usage: system-one outcome --truth <candidate_id> --source <verifier:x> (--decision-id id | --subject-ref ref [--question-id q])");
      const str = (k: string) => (typeof f[k] === "string" ? (f[k] as string) : null);
      console.log(JSON.stringify(new ShadowLedger(dir).recordOutcome({ truth: f.truth, source: f.source, decision_id: str("decision-id"), subject_ref: str("subject-ref"), question_id: str("question-id") })));
      return;
    }
    case "export": {
      // Ledger -> fine-tuning set: train/tune/cert/audit.jsonl per (bank, type, option count).
      const dir = typeof f["shadow-dir"] === "string" ? f["shadow-dir"] : (process.env.ALLTERNIT_S1_SHADOW_DIR ?? join(BASE_DIR, "shadow"));
      if (typeof f.out !== "string") throw new Error("usage: system-one export --out <dir> [--shadow-dir d] [--primitive bank] [--model ref] [--audit 0.05]");
      const { rows, summary } = buildExport(dir, {
        primitive: typeof f.primitive === "string" ? f.primitive : undefined, model: typeof f.model === "string" ? f.model : undefined,
        auditFraction: typeof f.audit === "string" ? Number(f.audit) : undefined,
      });
      writeExport(f.out, rows, summary);
      console.log(JSON.stringify({ out: f.out, ...summary }, null, 2));
      return;
    }
    case "canary": {
      // system-one canary <status|enable|grow|sync|rollback> [--bank b] [--report q26.json] [--budget N] [--audit-rate r]
      const sub = rest[0];
      const c = new CanaryController(typeof f.state === "string" ? f.state : (process.env.ALLTERNIT_S1_CANARY?.trim() || join(BASE_DIR, "canary.json")));
      const bank = typeof f.bank === "string" ? f.bank : undefined;
      const need = () => { if (!bank) throw new Error("--bank is required"); return bank; };
      if (sub === "status") console.log(JSON.stringify(bank ? c.bank(bank) ?? null : c.snapshot(), null, 2));
      else if (sub === "enable") {
        if (typeof f.report !== "string") throw new Error("canary enable needs --report <q26 report json> (the bank must be eligible)");
        const rep = JSON.parse(readFileSync(f.report, "utf8")) as Q26Report;
        console.log(JSON.stringify(c.enable(need(), rep, { budget_per_day: typeof f.budget === "string" ? Number(f.budget) : undefined, audit_rate: typeof f["audit-rate"] === "string" ? Number(f["audit-rate"]) : undefined }), null, 2));
      } else if (sub === "grow") console.log(JSON.stringify(c.grow(need()), null, 2));
      else if (sub === "rollback") { c.rollback(need(), "manual (cli)"); console.log(JSON.stringify(c.bank(need()), null, 2)); }
      else if (sub === "sync") {
        const dir = typeof f["shadow-dir"] === "string" ? f["shadow-dir"] : (process.env.ALLTERNIT_S1_SHADOW_DIR ?? join(BASE_DIR, "shadow"));
        console.log(JSON.stringify(c.syncFromRows(harvest(dir).rows), null, 2));
      } else throw new Error("usage: system-one canary <status|enable|grow|sync|rollback> [--bank b] [--report f] [--budget N]");
      return;
    }
    case "calibrate": {
      if (typeof f.tune === "string" || typeof f.cert === "string") {
        // Q26 (default gate): split A = --tune, split B = --cert (untouched).
        if (typeof f.tune !== "string" || typeof f.cert !== "string") throw new Error("usage: system-one calibrate --tune <jsonl> --cert <jsonl> [--policy p.json] [--report out.json] [--manifests path]");
        // Bundled defaults (q26-policy.json: banks with no incumbent decider), overridden per bank by --policy.
        const bundled = new URL("../q26-policy.json", import.meta.url);
        const defaults = existsSync(bundled) ? (JSON.parse(readFileSync(bundled, "utf8")) as Q26Policy) : {};
        const policy: Q26Policy = { ...defaults, ...(typeof f.policy === "string" ? (JSON.parse(readFileSync(f.policy, "utf8")) as Q26Policy) : {}) };
        const rep = runQ26(readDataset(f.tune), readDataset(f.cert), { policy, datasetRef: f.cert });
        const reportPath = typeof f.report === "string" ? f.report : `${f.cert}.q26-report.json`;
        const manifestsPath = typeof f.manifests === "string" ? f.manifests : process.env.ALLTERNIT_S1_MANIFESTS?.trim() || undefined;
        const passing = rep.bindings.filter((b) => b.passed && b.manifest).map((b) => b.manifest!);
        if (passing.length && manifestsPath) appendManifests(manifestsPath, passing);
        else if (passing.length) rep.notes.push("bindings passed but no manifests path (ALLTERNIT_S1_MANIFESTS / --manifests): nothing written");
        mkdirSync(dirname(reportPath), { recursive: true });
        writeFileSync(reportPath, JSON.stringify(rep, null, 2) + "\n");
        console.log(JSON.stringify({ gate: "Q26", report: reportPath, banks: rep.banks, bindings: rep.bindings.map((b) => ({ bank: b.bank, type: b.type, k: b.k, n_tune: b.n_tune, n_cert: b.n_cert, tau: b.tau, cert: b.cert, passed: b.passed, failures: b.failures })), notes: rep.notes }, null, 2));
        process.exitCode = Object.values(rep.banks).some((b) => b.eligible) ? 0 : 3;
        return;
      }
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
      console.error(`usage: system-one <serve|ask|models|route-model|harvest|outcome|export|calibrate|canary> (default port ${DEFAULT_PORT})`);
      break;
  }
  if (!["serve", "ask", "models", "route-model", "harvest", "outcome", "export", "calibrate", "canary"].includes(cmd ?? "")) process.exitCode = 2;
}

main().catch((e) => {
  if (e instanceof SystemOneError) console.error(JSON.stringify(e.body, null, 2));
  else console.error(`error: ${(e as Error).message}`);
  process.exitCode = 1;
});
