import { Client, StreamableHTTPClientTransport } from "@modelcontextprotocol/client";
import { MCP_APP_RESOURCE_MIME_TYPE, scanInput, type DirectoryFinding, type ScanInput, type ScanResource } from "@allternit/apps-server";

/** Connect to a running server, list tools/resources, read each ui:// resource, and run the directory scan rules. */
export async function liveScan(url: string, headers?: Record<string, string>): Promise<{ findings: DirectoryFinding[]; input: ScanInput }> {
  // auto: try 2026-07-28 (server/discover) first, fall back to the legacy initialize handshake.
  const client = new Client({ name: "allternit-cli", version: "0.1.0" }, { versionNegotiation: { mode: "auto" } });
  await client.connect(new StreamableHTTPClientTransport(new URL(url), headers ? { requestInit: { headers } } : undefined));
  try {
    const tools: ScanInput["tools"] = [];
    let cursor: string | undefined;
    for (let i = 0; i < 20; i++) {
      const page = await client.listTools(cursor ? { cursor } : undefined);
      tools.push(...(page.tools as ScanInput["tools"]));
      if (!(cursor = page.nextCursor)) break;
    }
    const resources: ScanResource[] = [];
    cursor = undefined;
    for (let i = 0; i < 20; i++) {
      // A server with no resources does not advertise the capability; that is "none", not a failure.
      if (!client.getServerCapabilities()?.resources) break;
      const page = await client.listResources(cursor ? { cursor } : undefined);
      for (const r of page.resources) resources.push({ uri: r.uri, name: r.name, mimeType: r.mimeType, meta: r._meta as Record<string, unknown> | undefined });
      if (!(cursor = page.nextCursor)) break;
    }
    const findings: DirectoryFinding[] = [];
    for (const r of resources) {
      if (!r.uri.startsWith("ui://")) continue;
      try {
        const read = await client.readResource({ uri: r.uri });
        const c = read.contents[0];
        r.meta = (c?._meta as Record<string, unknown> | undefined) ?? r.meta;
        r.mimeType = c?.mimeType ?? r.mimeType;
        const html = c && "text" in c ? c.text : "";
        if (typeof html !== "string" || !html.trim()) {
          findings.push({ severity: "error", code: "resource.empty", message: `${r.uri} returned no HTML.`, subject: `resource:${r.uri}` });
        } else if (c.mimeType !== MCP_APP_RESOURCE_MIME_TYPE) {
          findings.push({ severity: "error", code: "resource.mime", message: `${r.uri} read back as ${c.mimeType}, not ${MCP_APP_RESOURCE_MIME_TYPE}.`, subject: `resource:${r.uri}` });
        }
      } catch (err) {
        findings.push({ severity: "error", code: "resource.unreadable", message: `${r.uri} could not be read: ${err instanceof Error ? err.message : String(err)}`, subject: `resource:${r.uri}` });
      }
    }
    const input = { tools, resources };
    return { findings: [...scanInput(input), ...findings], input };
  } finally {
    await client.close();
  }
}
