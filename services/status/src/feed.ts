import type { IncidentsDoc as IncidentsResponse, Incident } from "../contract/types";

const STATUS_LABEL: Record<string, string> = {
  investigating: "Investigating", identified: "Identified", monitoring: "Monitoring", resolved: "Resolved",
};
const DAYS = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

const x = (s: string) =>
  s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;").replace(/'/g, "&apos;");

function rfc822(iso: string): string {
  const d = new Date(iso);
  if (isNaN(d.getTime())) return "Thu, 01 Jan 1970 00:00:00 GMT";
  const p = (n: number) => String(n).padStart(2, "0");
  return `${DAYS[d.getUTCDay()]}, ${p(d.getUTCDate())} ${MONTHS[d.getUTCMonth()]} ${d.getUTCFullYear()} ${p(d.getUTCHours())}:${p(d.getUTCMinutes())}:${p(d.getUTCSeconds())} GMT`;
}

function latestTs(i: Incident): string {
  let t = i.resolved_at ?? i.started_at;
  for (const u of i.updates) if (u.ts > t) t = u.ts;
  return t;
}

/** RSS 2.0 feed of incidents, newest first. */
export function renderFeed(incidents: IncidentsResponse, opts: { now?: string; siteUrl?: string } = {}): string {
  const site = (opts.siteUrl ?? "https://status.allternit.com").replace(/\/$/, "");
  const list = [...incidents.incidents].sort((a, b) => (latestTs(a) < latestTs(b) ? 1 : -1)).slice(0, 50);
  const built = rfc822(list.length ? latestTs(list[0]) : opts.now ?? "1970-01-01T00:00:00Z");
  const items = list.map((i) => {
    const ups = [...i.updates].sort((a, b) => (a.ts < b.ts ? 1 : -1));
    const body = ups.map((u) => `${STATUS_LABEL[u.status] ?? u.status} (${u.ts.replace("T", " ").replace(/:\d\dZ$/, " UTC")}): ${u.body}`).join("\n\n");
    const link = `${site}/#incident-${encodeURIComponent(i.id)}`;
    return `    <item>
      <title>${x(i.title)}${i.state === "resolved" ? " (resolved)" : ""}</title>
      <link>${x(link)}</link>
      <guid isPermaLink="false">${x(i.id)}@${x(latestTs(i))}</guid>
      <pubDate>${rfc822(latestTs(i))}</pubDate>
      <description>${x(body)}</description>
    </item>`;
  });
  return `<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:atom="http://www.w3.org/2005/Atom">
  <channel>
    <title>Allternit Status</title>
    <link>${x(site)}/</link>
    <description>Incidents reported on the Allternit status page.</description>
    <language>en</language>
    <lastBuildDate>${built}</lastBuildDate>
    <atom:link href="${x(site)}/feed.xml" rel="self" type="application/rss+xml"/>
${items.join("\n")}
  </channel>
</rss>
`;
}
