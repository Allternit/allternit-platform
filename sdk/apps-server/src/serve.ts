import { createServer, type IncomingMessage, type Server, type ServerResponse } from "node:http";
import type { AddressInfo } from "node:net";
import { Readable } from "node:stream";
import type { McpHttpHandler } from "@modelcontextprotocol/server";
import { MANIFEST_FILE } from "./manifest.js";
import type { AllternitApp } from "./app.js";

export interface ListenOptions {
  port?: number;
  host?: string;
  /** Overrides the app's own path. */
  path?: string;
  /** Out-of-band server errors (a tool that throws, a bad result). Default: logged to stderr. */
  onerror?: (error: Error) => void;
}

export interface RunningApp {
  url: string;
  port: number;
  close(): Promise<void>;
}

/** The app's dual-era MCP endpoint as a web-standard `fetch` handler. Same as `app.createHandler()`. */
export function createHandler(app: AllternitApp, options: { onerror?: (error: Error) => void } = {}): McpHttpHandler {
  return app.createHandler(options);
}

function toWebRequest(req: IncomingMessage, origin: string, signal: AbortSignal): Request {
  const headers = new Headers();
  for (const [k, v] of Object.entries(req.headers)) {
    if (Array.isArray(v)) for (const x of v) headers.append(k, x);
    else if (v !== undefined) headers.set(k, v);
  }
  const hasBody = req.method !== "GET" && req.method !== "HEAD";
  return new Request(new URL(req.url ?? "/", origin), {
    method: req.method,
    headers,
    signal,
    ...(hasBody ? { body: Readable.toWeb(req) as unknown as ReadableStream, duplex: "half" } : {}),
  } as RequestInit);
}

async function writeWebResponse(res: ServerResponse, response: Response): Promise<void> {
  const headers: Record<string, string> = {};
  response.headers.forEach((v, k) => (headers[k] = v));
  res.writeHead(response.status, headers);
  if (!response.body) return void res.end();
  for await (const chunk of response.body as unknown as AsyncIterable<Uint8Array>) res.write(chunk);
  res.end();
}

/**
 * Streamable HTTP, stateless and dual-era (see `createHandler`). Also answers
 * GET /healthz and GET /allternit.app.json so tools (and `allternit dev`) can
 * check it without an MCP handshake.
 */
export function listen(app: AllternitApp, options: ListenOptions = {}): Promise<RunningApp> {
  const path = options.path ?? app.input.path ?? "/mcp";
  const handler = createHandler(app, { onerror: options.onerror });
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
    const abort = new AbortController();
    res.on("close", () => abort.abort());
    try {
      const host = req.headers.host ?? "localhost";
      await writeWebResponse(res, await handler.fetch(toWebRequest(req, `http://${host}`, abort.signal)));
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
        close: async () => {
          await handler.close();
          await new Promise<void>((done) => http.close(() => done()));
        },
      });
    });
  });
}
