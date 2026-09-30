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
    default:
      console.error(`usage: system-one <serve|ask|models|route-model> (default port ${DEFAULT_PORT})`);
      process.exitCode = 2;
  }
}

main().catch((e) => {
  if (e instanceof SystemOneError) console.error(JSON.stringify(e.body, null, 2));
  else console.error(`error: ${(e as Error).message}`);
  process.exitCode = 1;
});
