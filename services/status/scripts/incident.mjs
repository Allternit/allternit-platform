#!/usr/bin/env node
// Status page admin from the terminal. Same API as /admin.
//
//   STATUS_ADMIN_TOKEN=... node scripts/incident.mjs <command> [options]
//
// Commands:
//   list                                   all incidents, drafts included
//   open --title T --components a,b --body B [--impact degraded|down|maintenance] [--status investigating|identified|monitoring]
//   update <id> [--title T] [--status S] [--body B] [--components a,b]
//   resolve <id> [--body B]
//   publish <id>                           publish an automatic draft
//   maintenance --title T --components a,b --body B --starts ISO --ends ISO
//
// Env: STATUS_URL (default https://status.allternit.com), STATUS_ADMIN_TOKEN (required).

const BASE = (process.env.STATUS_URL || "https://status.allternit.com").replace(/\/$/, "");
const TOKEN = process.env.STATUS_ADMIN_TOKEN || "";

function parse(argv) {
  const pos = [];
  const opt = {};
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a.startsWith("--")) {
      const k = a.slice(2);
      const v = argv[i + 1];
      if (v === undefined || v.startsWith("--")) throw new Error(`--${k} needs a value`);
      opt[k] = v;
      i++;
    } else pos.push(a);
  }
  return { pos, opt };
}

function need(opt, ...keys) {
  for (const k of keys) if (!opt[k]) throw new Error(`--${k} is required`);
}

const list = (s) => s.split(",").map((x) => x.trim()).filter(Boolean);

async function api(method, path, body) {
  const res = await fetch(BASE + path, {
    method,
    headers: { authorization: `Bearer ${TOKEN}`, "content-type": "application/json" },
    body: body ? JSON.stringify(body) : undefined,
  });
  const text = await res.text();
  let data;
  try {
    data = JSON.parse(text);
  } catch {
    data = { raw: text };
  }
  if (!res.ok) throw new Error(`${method} ${path}: HTTP ${res.status} ${data.error ?? text}`);
  return data;
}

function show(incidents) {
  if (!incidents.length) return console.log("No incidents.");
  for (const i of incidents) {
    const last = i.updates[i.updates.length - 1] ?? {};
    const flags = [i.state, i.impact, i.draft ? "DRAFT" : null, i.auto ? "auto" : "edited", i.kind === "maintenance" ? "maintenance" : null]
      .filter(Boolean)
      .join(", ");
    console.log(`${i.id}  ${i.title}`);
    console.log(`    ${flags} | ${i.component_ids.join(", ")} | started ${i.started_at}`);
    console.log(`    ${last.status}: ${last.body}`);
  }
}

async function main() {
  const [cmd, ...rest] = process.argv.slice(2);
  const { pos, opt } = parse(rest);
  if (!cmd || cmd === "help" || cmd === "--help") {
    console.log(
      "usage: incident.mjs list | open | update <id> | resolve <id> | publish <id> | maintenance  (see the header of this file)",
    );
    return;
  }
  if (!TOKEN) throw new Error("set STATUS_ADMIN_TOKEN");

  switch (cmd) {
    case "list":
      return show((await api("GET", "/api/admin/incidents")).incidents);
    case "open": {
      need(opt, "title", "components", "body");
      const r = await api("POST", "/api/admin/incidents", {
        schema: "allternit.admin.incident.create/v1",
        title: opt.title,
        impact: opt.impact ?? "degraded",
        component_ids: list(opt.components),
        body: opt.body,
        ...(opt.status ? { status: opt.status } : {}),
      });
      return console.log(`opened ${r.id}`);
    }
    case "update":
    case "resolve": {
      const id = pos[0];
      if (!id) throw new Error(`${cmd} needs an incident id`);
      const body = { schema: "allternit.admin.incident.patch/v1" };
      if (opt.title) body.title = opt.title;
      if (opt.body) body.body = opt.body;
      if (opt.components) body.component_ids = list(opt.components);
      if (cmd === "resolve") body.status = "resolved";
      else if (opt.status) body.status = opt.status;
      if (Object.keys(body).length === 1) throw new Error("nothing to change: pass --title, --status, --body or --components");
      await api("PATCH", `/api/admin/incidents/${encodeURIComponent(id)}`, body);
      return console.log(`${cmd === "resolve" ? "resolved" : "updated"} ${id}`);
    }
    case "publish": {
      const id = pos[0];
      if (!id) throw new Error("publish needs an incident id");
      await api("POST", `/api/admin/incidents/${encodeURIComponent(id)}/publish`);
      return console.log(`published ${id}`);
    }
    case "maintenance": {
      need(opt, "title", "components", "body", "starts", "ends");
      const r = await api("POST", "/api/admin/maintenance", {
        schema: "allternit.admin.maintenance.create/v1",
        title: opt.title,
        component_ids: list(opt.components),
        body: opt.body,
        starts_at: new Date(opt.starts).toISOString(),
        ends_at: new Date(opt.ends).toISOString(),
      });
      return console.log(`scheduled ${r.id}`);
    }
    default:
      throw new Error(`unknown command: ${cmd}`);
  }
}

main().catch((e) => {
  console.error(`incident.mjs: ${e.message}`);
  process.exit(1);
});
