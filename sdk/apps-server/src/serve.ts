import { createServer, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import { StreamableHTTPServerTransport } from "@modelcontextprotocol/sdk/server/streamableHttp.js";
import { MANIFEST_FILE } from "./manifest.js";
import type { AllternitApp } from "./app.js";

export interface ListenOptions {
  port?: number;
  host?: string;
  /** Overrides the app's own path. */
  path?: string;
}

export interface RunningApp {
  url: string;
  port: number;
  close(): Promise<void>;
}

/**
 * Streamable HTTP, stateless: a fresh server + transport per request. Also
 * answers GET /healthz and GET /allternit.app.json so tools (and `allternit
 * dev`) can check it without an MCP handshake.
 */
export function listen(app: AllternitApp, options: ListenOptions = {}): Promise<RunningApp> {
  const path = options.path ?? app.input.path ?? "/mcp";
  const http: Server = createServer(async (req, res) => {
    const url = (req.url ?? "/").split("?")[0];
    if (req.method === "GET" && url === "/healthz") {
      res.writeHead(200, { "content-type": "text/plain" }).end("ok");
      return;
    }
    if (req.method === "GET" && url === `/${MANIFEST_FILE}`) {
      res.writeHead(200, { "content-type": "application/json" }).end(JSON.stringify(app.manifest(), null, 2));
      return;
    }
    if (url !== path) {
      res.writeHead(404).end();
      return;
    }
    const server = app.createServer();
    const transport = new StreamableHTTPServerTransport({ sessionIdGenerator: undefined });
    res.on("close", () => {
      void transport.close();
      void server.close();
    });
    try {
      await server.connect(transport);
      await transport.handleRequest(req, res);
    } catch (err) {
      if (!res.headersSent) res.writeHead(500, { "content-type": "application/json" });
      res.end(JSON.stringify({ error: err instanceof Error ? err.message : "server error" }));
    }
  });
  return new Promise((resolve, reject) => {
    http.once("error", reject);
    http.listen(options.port ?? 3000, options.host ?? "127.0.0.1", () => {
      const port = (http.address() as AddressInfo).port;
      resolve({
        port,
        url: `http://${options.host ?? "localhost"}:${port}${path}`,
        close: () => new Promise((done) => http.close(() => done())),
      });
    });
  });
}
