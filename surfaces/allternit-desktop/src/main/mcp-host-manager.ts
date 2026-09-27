/**
 * Native MCP Host — manages Model Context Protocol servers in the Electron main process.
 *
 * Architecture: one McpClient per configured server, all running as child processes
 * via stdio transport. Works even when gizzi-code is down.
 *
 * Config: ~/.allternit/mcp-config.json  (Claude-compatible format)
 */

import * as fs from 'node:fs';
import * as path from 'node:path';
import * as child_process from 'node:child_process';
import { app, BrowserWindow } from 'electron';
import log from 'electron-log';

// ── Config shape (Claude Desktop compatible) ─────────────────────────────────

export interface McpServerConfig {
  command: string;
  args?: string[];
  env?: Record<string, string>;
  alwaysAllow?: string[];
  disabled?: boolean;
}

export interface McpConfig {
  mcpServers: Record<string, McpServerConfig>;
}

// ── Server lifecycle states ──────────────────────────────────────────────────

type ServerStatus = 'connecting' | 'running' | 'error' | 'disabled' | 'stopped';

interface ToolInfo {
  name: string;
  description?: string;
  inputSchema?: Record<string, unknown>;
}

interface ServerEntry {
  id: string;
  config: McpServerConfig;
  status: ServerStatus;
  tools: ToolInfo[];
  proc: child_process.ChildProcess | null;
  restartCount: number;
  errorMessage?: string;
  // Message buffer for the simple JSON-RPC over stdio protocol
  buffer: string;
  pendingRequests: Map<number, { resolve: (v: unknown) => void; reject: (e: Error) => void }>;
  nextRequestId: number;
}

const MAX_RESTARTS = 3;
const RESTART_BACKOFF_BASE_MS = 1000;
const TOOL_CALL_TIMEOUT_MS = 30_000;

// ── MCP JSON-RPC helpers ─────────────────────────────────────────────────────
// We implement a minimal MCP client without the full SDK to avoid adding a
// dependency. If @modelcontextprotocol/sdk is installed, this can be replaced.

function makeRequest(method: string, params?: unknown, id?: number) {
  return JSON.stringify({ jsonrpc: '2.0', id: id ?? null, method, params: params ?? {} }) + '\n';
}

class McpHostManager {
  private servers = new Map<string, ServerEntry>();
  private configPath: string;
  private configWatcher: fs.FSWatcher | null = null;

  constructor() {
    this.configPath = path.join(app.getPath('home'), '.allternit', 'mcp-config.json');
  }

  initialize(): void {
    const config = this.loadConfig();
    for (const [id, serverConfig] of Object.entries(config.mcpServers)) {
      if (!serverConfig.disabled) {
        this.startServer(id, serverConfig);
      } else {
        this.servers.set(id, this.makeEntry(id, serverConfig, 'disabled'));
      }
    }
    this.watchConfig();
    log.info(`[MCPHost] Initialized with ${this.servers.size} servers`);
  }

  listServers(): Array<{ id: string; status: ServerStatus; toolCount: number; error?: string }> {
    return [...this.servers.values()].map(e => ({
      id: e.id,
      status: e.status,
      toolCount: e.tools.length,
      error: e.errorMessage,
    }));
  }

  listTools(serverId?: string): Array<{ serverId: string } & ToolInfo> {
    const results: Array<{ serverId: string } & ToolInfo> = [];
    for (const [id, entry] of this.servers) {
      if (serverId && id !== serverId) continue;
      if (entry.status !== 'running') continue;
      for (const tool of entry.tools) {
        results.push({ serverId: id, ...tool });
      }
    }
    return results;
  }

  async callTool(serverId: string, toolName: string, args: unknown): Promise<unknown> {
    const entry = this.servers.get(serverId);
    if (!entry || entry.status !== 'running' || !entry.proc) {
      throw new Error(`MCP server "${serverId}" is not running`);
    }

    // Check alwaysAllow — if tool is not pre-approved, caller must have checked permission
    const config = entry.config;
    const allowed = config.alwaysAllow?.includes(toolName) ?? false;
    if (!allowed) {
      log.info(`[MCPHost] Tool call requires user approval: ${serverId}/${toolName}`);
      // Permission enforcement is handled by the IPC handler (shows dialog before calling here)
    }

    return this.sendRequest(entry, 'tools/call', { name: toolName, arguments: args });
  }

  async addServer(id: string, config: McpServerConfig): Promise<void> {
    if (this.servers.has(id)) {
      throw new Error(`Server "${id}" already exists`);
    }
    const fullConfig = this.loadConfig();
    fullConfig.mcpServers[id] = config;
    this.saveConfig(fullConfig);
    if (!config.disabled) this.startServer(id, config);
  }

  async removeServer(id: string): Promise<void> {
    this.stopServer(id);
    const fullConfig = this.loadConfig();
    delete fullConfig.mcpServers[id];
    this.saveConfig(fullConfig);
  }

  shutdown(): void {
    this.configWatcher?.close();
    for (const id of this.servers.keys()) this.stopServer(id);
  }

  // ── Private ────────────────────────────────────────────────────────────────

  private startServer(id: string, config: McpServerConfig, restartCount = 0): void {
    const entry = this.makeEntry(id, config, 'connecting', restartCount);
    this.servers.set(id, entry);

    try {
      const proc = child_process.spawn(config.command, config.args ?? [], {
        env: { ...process.env, ...(config.env ?? {}) } as NodeJS.ProcessEnv,
        stdio: ['pipe', 'pipe', 'pipe'],
        windowsHide: true,
      });

      entry.proc = proc;

      proc.stdout?.setEncoding('utf-8');
      proc.stdout?.on('data', (chunk: string) => this.handleStdout(entry, chunk));

      proc.stderr?.setEncoding('utf-8');
      proc.stderr?.on('data', (chunk: string) => {
        log.warn(`[MCPHost:${id}] stderr:`, chunk.trim());
      });

      proc.on('error', (err) => {
        log.error(`[MCPHost:${id}] spawn error:`, err);
        this.markError(entry, err.message);
      });

      proc.on('exit', (code) => {
        log.warn(`[MCPHost:${id}] exited (code ${code})`);
        entry.proc = null;
        this.rejectAllPending(entry, `Server "${id}" exited unexpectedly`);

        if (restartCount < MAX_RESTARTS) {
          const delay = RESTART_BACKOFF_BASE_MS * Math.pow(2, restartCount);
          log.info(`[MCPHost:${id}] Restarting in ${delay}ms (attempt ${restartCount + 1}/${MAX_RESTARTS})`);
          setTimeout(() => this.startServer(id, config, restartCount + 1), delay);
        } else {
          this.markError(entry, `Exceeded max restarts (${MAX_RESTARTS})`);
          this.broadcast('mcp:server-dead', { serverId: id });
        }
      });

      // Initialize handshake
      this.initializeServer(entry);

    } catch (err) {
      this.markError(entry, (err as Error).message);
    }
  }

  private async initializeServer(entry: ServerEntry): Promise<void> {
    try {
      // Send initialize request
      await this.sendRequest(entry, 'initialize', {
        protocolVersion: '2024-11-05',
        capabilities: { roots: {}, sampling: {} },
        clientInfo: { name: 'allternit-desktop', version: '1.0.0' },
      });

      // Send initialized notification
      if (entry.proc?.stdin) {
        entry.proc.stdin.write(makeRequest('notifications/initialized'));
      }

      // Discover tools
      const toolsResult = await this.sendRequest(entry, 'tools/list', {}) as { tools: ToolInfo[] };
      entry.tools = toolsResult.tools ?? [];
      entry.status = 'running';

      log.info(`[MCPHost:${entry.id}] Running with ${entry.tools.length} tools`);
      this.broadcast('mcp:server-ready', { serverId: entry.id, tools: entry.tools });

    } catch (err) {
      this.markError(entry, (err as Error).message);
    }
  }

  private handleStdout(entry: ServerEntry, chunk: string): void {
    entry.buffer += chunk;
    const lines = entry.buffer.split('\n');
    entry.buffer = lines.pop() ?? '';

    for (const line of lines) {
      if (!line.trim()) continue;
      try {
        const msg = JSON.parse(line) as { id?: number; result?: unknown; error?: { message: string } };
        if (msg.id !== undefined && entry.pendingRequests.has(msg.id)) {
          const req = entry.pendingRequests.get(msg.id)!;
          entry.pendingRequests.delete(msg.id);
          if (msg.error) req.reject(new Error(msg.error.message));
          else req.resolve(msg.result);
        }
      } catch {
        log.warn(`[MCPHost:${entry.id}] Unparseable stdout:`, line.slice(0, 200));
      }
    }
  }

  private sendRequest(entry: ServerEntry, method: string, params: unknown): Promise<unknown> {
    return new Promise((resolve, reject) => {
      if (!entry.proc?.stdin) {
        reject(new Error('Server process stdin not available'));
        return;
      }

      const id = entry.nextRequestId++;
      const timer = setTimeout(() => {
        entry.pendingRequests.delete(id);
        reject(new Error(`Request "${method}" timed out after ${TOOL_CALL_TIMEOUT_MS}ms`));
      }, TOOL_CALL_TIMEOUT_MS);

      entry.pendingRequests.set(id, {
        resolve: (v) => { clearTimeout(timer); resolve(v); },
        reject: (e) => { clearTimeout(timer); reject(e); },
      });

      entry.proc.stdin.write(makeRequest(method, params, id));
    });
  }

  private stopServer(id: string): void {
    const entry = this.servers.get(id);
    if (!entry) return;
    this.rejectAllPending(entry, `Server "${id}" stopped`);
    if (entry.proc) {
      try { entry.proc.stdin?.end(); } catch { /* ignore */ }
      setTimeout(() => { try { entry.proc?.kill('SIGTERM'); } catch { /* ignore */ } }, 500);
    }
    entry.status = 'stopped';
    entry.proc = null;
  }

  private rejectAllPending(entry: ServerEntry, reason: string): void {
    for (const req of entry.pendingRequests.values()) req.reject(new Error(reason));
    entry.pendingRequests.clear();
  }

  private markError(entry: ServerEntry, message: string): void {
    entry.status = 'error';
    entry.errorMessage = message;
    this.broadcast('mcp:server-error', { serverId: entry.id, error: message });
  }

  private makeEntry(id: string, config: McpServerConfig, status: ServerStatus, restartCount = 0): ServerEntry {
    return { id, config, status, tools: [], proc: null, restartCount, buffer: '', pendingRequests: new Map(), nextRequestId: 1 };
  }

  private loadConfig(): McpConfig {
    try {
      if (fs.existsSync(this.configPath)) {
        return JSON.parse(fs.readFileSync(this.configPath, 'utf-8')) as McpConfig;
      }
    } catch (err) {
      log.warn('[MCPHost] Failed to load config:', err);
    }
    return { mcpServers: {} };
  }

  private saveConfig(config: McpConfig): void {
    try {
      const dir = path.dirname(this.configPath);
      if (!fs.existsSync(dir)) fs.mkdirSync(dir, { recursive: true });
      fs.writeFileSync(this.configPath, JSON.stringify(config, null, 2));
    } catch (err) {
      log.warn('[MCPHost] Failed to save config:', err);
    }
  }

  private watchConfig(): void {
    try {
      this.configWatcher = fs.watch(this.configPath, { persistent: false }, () => {
        log.info('[MCPHost] Config changed — reloading');
        const config = this.loadConfig();
        // Start any new servers
        for (const [id, cfg] of Object.entries(config.mcpServers)) {
          if (!this.servers.has(id) && !cfg.disabled) this.startServer(id, cfg);
        }
      });
    } catch { /* file may not exist yet */ }
  }

  private broadcast(channel: string, data: unknown): void {
    for (const win of BrowserWindow.getAllWindows()) {
      if (!win.isDestroyed()) win.webContents.send(channel, data);
    }
  }
}

export const mcpHostManager = new McpHostManager();
