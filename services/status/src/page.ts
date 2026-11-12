import type {
  StatusDoc as StatusResponse,
  HistoryDoc as HistoryResponse,
  IncidentsDoc as IncidentsResponse,
  Status,
  Incident,
  Component,
} from "../contract/types";

const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const esc = (s: unknown) =>
  String(s).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;").replace(/'/g, "&#39;");

const STATUS_TEXT: Record<Status, string> = {
  operational: "Operational", degraded: "Degraded", down: "Down", maintenance: "Maintenance", unknown: "No data",
};
const BANNER: Record<Status, string> = {
  operational: "All systems operational",
  degraded: "Some systems degraded",
  down: "Major outage",
  maintenance: "Maintenance",
  unknown: "Status unavailable",
};
const INCIDENT_STATUS: Record<string, string> = {
  investigating: "Investigating", identified: "Identified", monitoring: "Monitoring", resolved: "Resolved",
};
const IMPACT_TEXT: Record<string, string> = { degraded: "Degraded", down: "Down", maintenance: "Maintenance" };

/** Shape icon per status so colour is never the only signal. */
function icon(s: Status, cls = ""): string {
  const a = `class="${cls}" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false"`;
  switch (s) {
    case "operational": return `<svg ${a}><circle cx="12" cy="12" r="9.5"/><path d="M7.5 12.5l3 3 6-6.5"/></svg>`;
    case "degraded": return `<svg ${a}><path d="M12 3.5l9.5 16.5h-19z"/><path d="M12 10v4.5M12 17.4v.1"/></svg>`;
    case "down": return `<svg ${a}><rect x="3" y="3" width="18" height="18" rx="4"/><path d="M8.5 8.5l7 7M15.5 8.5l-7 7"/></svg>`;
    case "maintenance": return `<svg ${a}><path d="M12 2.5l9.5 9.5-9.5 9.5L2.5 12z"/><path d="M12 8v4.5l2.5 1.5"/></svg>`;
    default: return `<svg ${a}><circle cx="12" cy="12" r="9.5" stroke-dasharray="3 3.5"/></svg>`;
  }
}

const MARK = `<svg viewBox="0 0 50 50" aria-hidden="true" focusable="false"><g class="mark-ink"><rect x="20.75" y="0.75" width="8.5" height="8.5" rx="1.5"/><rect x="10.75" y="10.75" width="8.5" height="8.5" rx="1.5"/><rect x="30.75" y="10.75" width="8.5" height="8.5" rx="1.5"/><rect x="0.75" y="20.75" width="8.5" height="8.5" rx="1.5"/><rect x="10.75" y="20.75" width="8.5" height="8.5" rx="1.5"/><rect x="30.75" y="20.75" width="8.5" height="8.5" rx="1.5"/><rect x="40.75" y="20.75" width="8.5" height="8.5" rx="1.5"/><rect x="0.75" y="30.75" width="8.5" height="8.5" rx="1.5"/><rect x="40.75" y="30.75" width="8.5" height="8.5" rx="1.5"/><rect x="0.75" y="40.75" width="8.5" height="8.5" rx="1.5"/><rect x="40.75" y="40.75" width="8.5" height="8.5" rx="1.5"/></g><rect class="mark-core" x="20.75" y="20.75" width="8.5" height="8.5" rx="1.5"/></svg>`;

const FAVICON = "data:image/svg+xml," + encodeURIComponent(
  `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 50 50"><g fill="#141413"><rect x="20.75" y="0.75" width="8.5" height="8.5" rx="1.5"/><rect x="10.75" y="10.75" width="8.5" height="8.5" rx="1.5"/><rect x="30.75" y="10.75" width="8.5" height="8.5" rx="1.5"/><rect x="0.75" y="20.75" width="8.5" height="8.5" rx="1.5"/><rect x="10.75" y="20.75" width="8.5" height="8.5" rx="1.5"/><rect x="30.75" y="20.75" width="8.5" height="8.5" rx="1.5"/><rect x="40.75" y="20.75" width="8.5" height="8.5" rx="1.5"/><rect x="0.75" y="30.75" width="8.5" height="8.5" rx="1.5"/><rect x="40.75" y="30.75" width="8.5" height="8.5" rx="1.5"/><rect x="0.75" y="40.75" width="8.5" height="8.5" rx="1.5"/><rect x="40.75" y="40.75" width="8.5" height="8.5" rx="1.5"/></g><rect x="20.75" y="20.75" width="8.5" height="8.5" rx="1.5" fill="#D97757"/></svg>`,
);

const GROUPS: { title: string; ids: string[] }[] = [
  { title: "Core", ids: ["website", "app", "mobile", "signin", "api"] },
  { title: "Computers and devices", ids: ["cloud-computers", "phone-push"] },
  { title: "Other services", ids: ["mail", "console", "docs", "services", "downloads", "other"] },
];

const p2 = (n: number) => String(n).padStart(2, "0");
function fmtDate(d: Date): string {
  return `${MONTHS[d.getUTCMonth()]} ${d.getUTCDate()}, ${d.getUTCFullYear()}`;
}
function fmtTime(iso: string): string {
  const d = new Date(iso);
  if (isNaN(d.getTime())) return esc(iso);
  return `${fmtDate(d)}, ${p2(d.getUTCHours())}:${p2(d.getUTCMinutes())} UTC`;
}
/** Floors so 99.996% never shows as 100%. */
function fmtUptime(u: number | null): string {
  if (u == null) return "No uptime data yet";
  let s = (Math.floor(u * 10000) / 100).toFixed(2);
  if (s.includes(".")) s = s.replace(/0+$/, "").replace(/\.$/, "");
  return `${s}% uptime`;
}

function bar(c: Component, history: HistoryResponse, endDate: Date): string {
  const days = new Map((history.components.find((h) => h.id === c.id)?.days ?? []).map((d) => [d.date, d]));
  const N = 90;
  const cells: string[] = [];
  for (let i = 0; i < N; i++) {
    const d = new Date(Date.UTC(endDate.getUTCFullYear(), endDate.getUTCMonth(), endDate.getUTCDate() - (N - 1 - i)));
    const key = `${d.getUTCFullYear()}-${p2(d.getUTCMonth() + 1)}-${p2(d.getUTCDate())}`;
    const h = days.get(key);
    const st: Status | null = h?.status ?? null;
    const label = st == null
      ? `${fmtDate(d)}: No data`
      : `${fmtDate(d)}: ${STATUS_TEXT[st]}${h?.uptime != null ? `, ${fmtUptime(h.uptime)}` : ""}`;
    const pos = i < 12 ? " l" : i > N - 13 ? " r" : "";
    const left = i < 12 ? (i / N) * 100 : i > N - 13 ? ((i + 1) / N) * 100 : ((i + 0.5) / N) * 100;
    const stop = st && st !== "operational" ? 0 : -1;
    cells.push(
      `<span class="cell s-${st ?? "none"}" tabindex="${stop}" role="img" aria-label="${esc(label)}"><span class="tip${pos}" style="left:${left.toFixed(2)}%" aria-hidden="true">${esc(label)}</span></span>`,
    );
  }
  return `<div class="bar" tabindex="0" role="group" aria-label="${esc(c.name)}: last 90 days, one cell per day. Use the left and right arrow keys to read each day.">${cells.join("")}</div><div class="ends" aria-hidden="true"><span>90 days ago</span><span>Today</span></div>`;
}

function coverage(c: Component, history: HistoryResponse, end: Date): string {
  const N = 90;
  const start = new Date(Date.UTC(end.getUTCFullYear(), end.getUTCMonth(), end.getUTCDate() - (N - 1)));
  const startKey = `${start.getUTCFullYear()}-${p2(start.getUTCMonth() + 1)}-${p2(start.getUTCDate())}`;
  const have = (history.components.find((h) => h.id === c.id)?.days ?? [])
    .filter((d) => d.uptime != null && d.date >= startKey)
    .sort((a, b) => (a.date < b.date ? -1 : 1));
  if (!have.length) return "No data yet";
  const avg = have.reduce((t, d) => t + (d.uptime as number), 0) / have.length;
  const [, m, dd] = have[0].date.split("-").map(Number);
  const span = have[0].date <= startKey ? "last 90 days" : `since ${MONTHS[m - 1]} ${dd}`;
  return `${fmtUptime(avg)} ${span}, measured by our checks`;
}

function component(c: Component, history: HistoryResponse, end: Date): string {
  const cov = coverage(c, history, end);
  return `<li class="comp" id="c-${esc(c.id)}"><div class="row"><div><div class="name">${esc(c.name)}</div><div class="meta">${esc(cov)}</div></div><div class="st t-${c.status}">${icon(c.status)}<span>${STATUS_TEXT[c.status]}</span></div></div>${bar(c, history, end)}</li>`;
}

function updates(i: Incident): string {
  const ups = [...i.updates].sort((a, b) => (a.ts < b.ts ? 1 : -1));
  return `<ol class="tl">${ups.map((u) => `<li><b>${esc(INCIDENT_STATUS[u.status] ?? u.status)}</b><time datetime="${esc(u.ts)}">${fmtTime(u.ts)}</time><p>${esc(u.body)}</p></li>`).join("")}</ol>`;
}

function incidentHead(i: Incident, names: Map<string, string>): string {
  const affected = i.component_ids.map((id) => names.get(id) ?? id).join(", ");
  const when = i.resolved_at ? `Started ${fmtTime(i.started_at)}. Resolved ${fmtTime(i.resolved_at)}.` : `Started ${fmtTime(i.started_at)}.`;
  return `<h3>${esc(i.title)}</h3><div class="sub">${IMPACT_TEXT[i.impact] ?? esc(i.impact)}${affected ? `: ${esc(affected)}` : ""}. ${when}</div>`;
}

const SCRIPT = `(function(){
var t=document.querySelectorAll("time[data-rel]");
function rel(){var n=Date.now();t.forEach(function(e){var s=Math.round((n-new Date(e.getAttribute("datetime")).getTime())/1000);if(!(s>=0))return;var r=s<90?"less than a minute ago":s<5400?Math.round(s/60)+" minutes ago":Math.round(s/3600)+" hours ago";var o=e.querySelector(".rel");if(!o){o=document.createElement("span");o.className="rel";e.appendChild(o)}o.textContent=" ("+r+")"})}
rel();setInterval(rel,30000);
document.querySelectorAll(".bar").forEach(function(b){b.addEventListener("keydown",function(e){
var c=[].slice.call(b.querySelectorAll(".cell"));var i=c.indexOf(document.activeElement);
if(e.key==="ArrowLeft"||e.key==="ArrowRight"){e.preventDefault();var n=i<0?c.length-1:Math.max(0,Math.min(c.length-1,i+(e.key==="ArrowLeft"?-1:1)));c[n].focus()}
else if(e.key==="Home"){e.preventDefault();c[0].focus()}else if(e.key==="End"){e.preventDefault();c[c.length-1].focus()}});});
})();`;

export function renderPage(
  status: StatusResponse,
  history: HistoryResponse,
  incidents: IncidentsResponse,
  opts: { now?: string; historyHref?: string } = {},
): string {
  const nowIso = opts.now ?? status.last_checked_at;
  const nowD = new Date(nowIso);
  const now = isNaN(nowD.getTime()) ? new Date(status.last_checked_at) : nowD;
  const names = new Map(status.components.map((c) => [c.id, c.name]));
  const byId = new Map(status.components.map((c) => [c.id, c]));
  const used = new Set<string>();
  const groups = GROUPS.map((g) => ({ title: g.title, items: g.ids.map((id) => byId.get(id)).filter((c): c is Component => !!c) }));
  for (const g of groups) for (const c of g.items) used.add(c.id);
  const rest = status.components.filter((c) => !used.has(c.id));
  if (rest.length) groups.push({ title: "More", items: rest });

  const open = incidents.incidents.filter((i) => i.state === "open");
  const cutoff = now.getTime() - 14 * 86400000;
  const past = incidents.incidents
    .filter((i) => i.state === "resolved" && i.resolved_at && new Date(i.resolved_at).getTime() >= cutoff)
    .sort((a, b) => ((a.resolved_at ?? "") < (b.resolved_at ?? "") ? 1 : -1));
  const histHref = opts.historyHref ?? "/api/v1/incidents.json";
  const banner = status.overall;

  const openHtml = open.length
    ? `<h2>Open incidents</h2>${open.map((i) => `<article class="card open i-${esc(i.impact)}" id="incident-${esc(i.id)}">${incidentHead(i, names)}${updates(i)}</article>`).join("")}`
    : "";
  const pastHtml = `<h2 id="past-incidents">Past incidents, last 14 days</h2>${
    past.length
      ? past.map((i) => `<details class="card" id="incident-${esc(i.id)}"><summary>${incidentHead(i, names)}</summary>${updates(i)}</details>`).join("")
      : `<p class="empty">No incidents in the last 14 days.</p>`
  }<p class="more"><a href="${esc(histHref)}">Incident history</a></p>`;

  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<meta name="color-scheme" content="light dark">
<title>Allternit Status</title>
<meta name="description" content="Current status of Allternit services, measured by our own checks.">
<link rel="icon" type="image/svg+xml" href="${FAVICON}">
<link rel="alternate" type="application/rss+xml" title="Allternit Status incidents" href="/feed.xml">
<link rel="stylesheet" href="/status.css">
</head>
<body>
<header class="top"><div class="wrap">
<a class="brand" href="https://allternit.com" aria-label="Allternit">${MARK}<span>Allternit Status</span></a>
<nav aria-label="Site"><a href="https://allternit.com/support">Support</a></nav>
</div></header>
<main class="wrap">
<section class="banner b-${banner}" role="status">${icon(banner, "ico")}<div><h1>${BANNER[banner]}</h1><p>Last checked <time datetime="${esc(status.last_checked_at)}" data-rel>${fmtTime(status.last_checked_at)}</time></p></div></section>
${openHtml}
<h2>Components</h2>
<p class="lead">Each cell is one day (UTC). Uptime is measured by our checks, over the days they have run (up to the last 90).</p>
${groups.map((g) => `<section class="group" aria-label="${esc(g.title)}"><h3>${esc(g.title)}</h3><ul>${g.items.map((c) => component(c, history, now)).join("")}</ul></section>`).join("\n")}
${pastHtml}
</main>
<footer><div class="wrap">
<a href="https://allternit.com">allternit.com</a>
<a href="https://allternit.com/support">Support</a>
<a href="/feed.xml">RSS feed</a>
<a href="/api/v1/status.json">JSON API</a>
</div></footer>
<script>${SCRIPT}</script>
</body>
</html>
`;
}
