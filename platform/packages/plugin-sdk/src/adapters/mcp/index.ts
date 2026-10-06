/**
 * MCP (Model Context Protocol) Adapter
 * 
 * Universal adapter for any MCP-compatible client:
 * - Claude Desktop
 * - Cursor (with MCP support)
 * - Any tool implementing MCP spec
 * 
 * Maps PluginHost interface to MCP tools/resources.
 */

import { 
  Plugin, 
  PluginHost, 
  Adapter, 
  AdapterInstance, 
  AdapterEndpoint,
  ToolResult,
  ToolMetadata,
  LlmOptions,
  StreamHandler,
  ImageData,
  ChartData,
  ChartOptions,
  PanelContent,
  FormSchema,
  FormData,
  Logger,
  PanelHandle,
  SessionContext,
  LlmCapabilities,
  ToolRegistry,
  UiCapabilities,
  StorageBackend,
} from '../../types';

// Official MCP SDK v2: serves MCP 2026-07-28 (server/discover, per-request
// _meta) and the legacy initialize handshake from one factory.
import { Server, ProtocolError, ProtocolErrorCode, type Tool, type Transport } from '@modelcontextprotocol/server';
import { serveStdio } from '@modelcontextprotocol/server/stdio';

export interface McpAdapterConfig {
  /** Transport type */
  transport?: 'stdio' | 'http' | 'websocket';
  
  /** For HTTP transport */
  port?: number;
  host?: string;
  
  /** Capabilities to expose */
  exposeTools?: boolean;
  exposeResources?: boolean;
  exposePrompts?: boolean;
  
  /** Custom server info */
  serverName?: string;
  serverVersion?: string;
}

/**
 * Universal MCP Adapter
 * 
 * Works with ANY MCP client - Claude Desktop, Cursor, or custom implementations.
 */
export class McpAdapter implements Adapter {
  readonly name = 'mcp';
  readonly version = '1.0.0';
  
  canActivate(): boolean {
    // MCP works anywhere Node.js runs
    return typeof process !== 'undefined';
  }
  
  async initialize(plugin: Plugin, config: McpAdapterConfig = {}): Promise<AdapterInstance> {
    return new McpAdapterInstance(plugin, config);
  }
}

class McpAdapterInstance implements AdapterInstance {
  private handle?: { close(): Promise<void> };
  private host: McpPluginHost;
  private plugin: Plugin;
  
  constructor(plugin: Plugin, private config: McpAdapterConfig) {
    this.plugin = plugin;
    this.host = new McpPluginHost(plugin);
  }

  /** A fresh MCP server for one connection (serveStdio calls this per era). */
  createServer(): Server {
    const config = this.config;
    const plugin = this.plugin;
    const server = new Server(
      {
        name: config.serverName || plugin.manifest.id,
        version: config.serverVersion || plugin.manifest.version,
      },
      {
        capabilities: {
          ...(config.exposeTools !== false ? { tools: {} } : {}),
          ...(config.exposeResources !== false ? { resources: {} } : {}),
        },
      }
    );
    
    this.setupHandlers(server);
    return server;
  }
  
  private setupHandlers(server: Server): void {
    // List available tools (one per plugin function)
    if (this.config.exposeTools !== false) server.setRequestHandler('tools/list', async () => {
      const tools: Tool[] = this.plugin.manifest.provides.functions.map(fn => ({
        name: fn.name,
        description: fn.description,
        inputSchema: fn.parameters as unknown as Tool['inputSchema'],
      }));
      
      // Add meta-tool for plugin info
      tools.push({
        name: '_plugin_info',
        description: `Get information about the ${this.plugin.manifest.name} plugin`,
        inputSchema: {
          type: 'object',
          properties: {},
        },
      });
      
      return { tools };
    });
    
    // Execute tool
    if (this.config.exposeTools !== false) server.setRequestHandler('tools/call', async (request) => {
      const { name, arguments: args } = request.params;
      
      try {
        if (name === '_plugin_info') {
          return {
            content: [
              {
                type: 'text',
                text: this.formatPluginInfo(),
              },
            ],
          };
        }
        
        // Find the function
        const fn = this.plugin.manifest.provides.functions.find(f => f.name === name);
        if (!fn) {
          throw new ProtocolError(
            ProtocolErrorCode.MethodNotFound,
            `Unknown tool: ${name}`
          );
        }
        
        // Execute via plugin
        const result = await this.plugin.execute(name, args || {}, {
          executionId: `mcp_${Date.now()}`,
          startedAt: new Date(),
          metadata: { source: 'mcp' },
        });
        
        // Format result for MCP
        const content: any[] = [];
        
        if (result.content) {
          content.push({
            type: 'text',
            text: result.content,
          });
        }
        
        // Handle artifacts
        if (result.artifacts) {
          for (const artifact of result.artifacts) {
            if (artifact.type === 'image') {
              content.push({
                type: 'image',
                data: artifact.content.toString(),
                mimeType: artifact.format,
              });
            } else if (artifact.type === 'code') {
              content.push({
                type: 'text',
                text: `\`\`\`${artifact.format || ''}\n${artifact.content}\n\`\`\``,
              });
            }
          }
        }
        
        return { content };
        
      } catch (error) {
        return {
          content: [
            {
              type: 'text',
              text: `Error: ${error instanceof Error ? error.message : String(error)}`,
            },
          ],
          isError: true,
        };
      }
    });
    
    // List resources (plugin documentation)
    if (this.config.exposeResources !== false) server.setRequestHandler('resources/list', async () => {
      return {
        resources: [
          {
            uri: `plugin://${this.plugin.manifest.id}/manifest`,
            name: 'Plugin Manifest',
            mimeType: 'application/json',
          },
          {
            uri: `plugin://${this.plugin.manifest.id}/readme`,
            name: 'Documentation',
            mimeType: 'text/markdown',
          },
        ],
      };
    });
    
    // Read resource
    if (this.config.exposeResources !== false) server.setRequestHandler('resources/read', async (request) => {
      const uri = request.params.uri;
      
      if (uri === `plugin://${this.plugin.manifest.id}/manifest`) {
        return {
          contents: [
            {
              uri,
              mimeType: 'application/json',
              text: JSON.stringify(this.plugin.manifest, null, 2),
            },
          ],
        };
      }
      
      if (uri === `plugin://${this.plugin.manifest.id}/readme`) {
        return {
          contents: [
            {
              uri,
              mimeType: 'text/markdown',
              text: this.generateReadme(),
            },
          ],
        };
      }
      
      throw new ProtocolError(
        ProtocolErrorCode.InvalidRequest,
        `Unknown resource: ${uri}`
      );
    });
  }
  
  private formatPluginInfo(): string {
    const m = this.plugin.manifest;
    return `
# ${m.name} v${m.version}

${m.description}

## Available Functions
${m.provides.functions.map(fn => `- **${fn.name}**: ${fn.description}`).join('\n')}

## Required Tools
${m.requires.tools?.map(t => typeof t === 'string' ? `- ${t}` : `- ${t.tool} (${t.required ? 'required' : 'optional'})`).join('\n') || 'None'}

## Usage Example
\`\`\`
Use ${m.provides.functions[0]?.name} with parameters
\`\`\`
`.trim();
  }
  
  private generateReadme(): string {
    return `
# ${this.plugin.manifest.name}

${this.plugin.manifest.description}

## Installation

Add to your MCP client configuration:

\`\`\`json
{
  "mcpServers": {
    "${this.plugin.manifest.id}": {
      "command": "npx",
      "args": ["-y", "@allternit/plugin-runtime", "run", "${this.plugin.manifest.id}"],
      "env": {
        "ALLTERNIT_PLUGIN_ID": "${this.plugin.manifest.id}"
      }
    }
  }
}
\`\`\`

## Functions

${this.plugin.manifest.provides.functions.map(fn => `
### ${fn.name}

${fn.description}

**Parameters:**
${Object.entries(fn.parameters.properties).map(([key, val]: [string, any]) => `- \`${key}\`: ${val.description}${fn.parameters.required?.includes(key) ? ' (required)' : ''}`).join('\n')}
`).join('\n---\n')}
`.trim();
  }
  
  /** Serve over this process's stdio (or `transport`, for tests). */
  async start(transport?: Transport): Promise<void> {
    this.handle = serveStdio(() => this.createServer(), transport ? { transport } : {});
    
    // Initialize plugin with our host
    await this.plugin.initialize(this.host);
    
    console.error(`MCP server running for ${this.plugin.manifest.id}`);
  }
  
  async stop(): Promise<void> {
    await this.plugin.destroy?.();
    await this.handle?.close();
    this.handle = undefined;
  }
  
  getEndpoint(): AdapterEndpoint {
    return {
      type: 'stdio',
      command: ['npx', '-y', '@allternit/plugin-runtime', 'run', this.plugin.manifest.id],
    };
  }
}

/**
 * PluginHost implementation for MCP environment
 * 
 * Provides best-effort mapping of PluginHost to MCP capabilities.
 * Gracefully degrades when MCP doesn't support certain features.
 */
class McpPluginHost implements PluginHost {
  readonly platform = 'mcp';
  readonly version = '1.0.0';
  readonly config: Record<string, any> = {};
  
  llm: LlmCapabilities;
  tools: ToolRegistry;
  ui: UiCapabilities;
  storage: StorageBackend;
  session: SessionContext;
  logger: Logger;
  
  constructor(private plugin: Plugin) {
    this.llm = new McpLlmCapabilities();
    this.tools = new McpToolRegistry();
    this.ui = new McpUiCapabilities();
    this.storage = new McpStorageBackend();
    this.session = {
      id: `mcp_${Date.now()}`,
      startedAt: new Date(),
      metadata: {},
      async getHistory() { return []; },
    };
    this.logger = {
      debug: (...args) => console.error('[debug]', ...args),
      info: (...args) => console.error('[info]', ...args),
      warn: (...args) => console.error('[warn]', ...args),
      error: (...args) => console.error('[error]', ...args),
    };
  }
}

// MCP implementations of PluginHost interfaces

class McpLlmCapabilities implements LlmCapabilities {
  readonly models = ['claude', 'gpt'];
  readonly defaultModel = 'claude';
  readonly maxContextWindow = 200000;
  
  async complete(prompt: string, options?: LlmOptions): Promise<string> {
    // In MCP, we rely on the host LLM
    // Return prompt as-is; host will process
    console.error('[MCP] LLM complete called (delegated to host)');
    return `[LLM output would be generated by MCP host for: ${prompt.slice(0, 100)}...]`;
  }
  
  async stream(prompt: string, handler: StreamHandler): Promise<void> {
    console.error('[MCP] LLM stream called (delegated to host)');
    handler('[Streaming not available in MCP adapter]');
  }
  
  async countTokens(text: string): Promise<number> {
    // Rough estimate
    return Math.ceil(text.length / 4);
  }
}

class McpToolRegistry implements ToolRegistry {
  private tools: Map<string, ToolMetadata> = new Map();
  
  constructor() {
    // Register MCP-native tools
    this.registerTool('web_search', 'Web Search', 'Search the web', {
      type: 'object',
      properties: { query: { type: 'string', description: 'Search query' } },
      required: ['query'],
    });
    this.registerTool('file_read', 'Read File', 'Read file contents', {
      type: 'object',
      properties: { path: { type: 'string', description: 'File path' } },
      required: ['path'],
    });
  }
  
  private registerTool(id: string, name: string, description: string, parameters: any): void {
    this.tools.set(id, { id: id as any, name, description, parameters });
  }
  
  has(toolId: string): boolean {
    return this.tools.has(toolId);
  }
  
  async execute<T>(toolId: string, params: Record<string, any>): Promise<ToolResult<T>> {
    console.error(`[MCP] Tool execution requested: ${toolId}`, params);
    
    // In MCP, tools are provided by the host
    // We indicate that we need this tool
    return {
      success: false,
      error: {
        code: 'TOOL_NOT_IMPLEMENTED',
        message: `Tool ${toolId} must be provided by MCP host. Add it to capabilities.`,
        retryable: false,
      },
    };
  }
  
  metadata(toolId: string): ToolMetadata | undefined {
    return this.tools.get(toolId);
  }
  
  list(): ToolMetadata[] {
    return Array.from(this.tools.values());
  }
}

class McpUiCapabilities implements UiCapabilities {
  private outputs: string[] = [];
  
  renderMarkdown(content: string): void {
    this.outputs.push(content);
    console.error('[MCP] Markdown output:', content.slice(0, 200));
  }
  
  renderCode(content: string, language: string): void {
    this.outputs.push(`\`\`\`${language}\n${content}\n\`\`\``);
    console.error('[MCP] Code output:', language);
  }
  
  renderImage(data: ImageData): void {
    console.error('[MCP] Image output (pass to MCP resources)');
  }
  
  renderChart(type: string, data: ChartData): void {
    console.error('[MCP] Chart output (render as markdown table)');
    this.renderTable(
      ['Label', ...data.datasets.map(d => d.label)],
      data.labels.map((label, i) => [
        label,
        ...data.datasets.map(d => d.data[i].toString()),
      ])
    );
  }
  
  renderTable(headers: string[], rows: any[][]): void {
    const table = [
      '| ' + headers.join(' | ') + ' |',
      '| ' + headers.map(() => '---').join(' | ') + ' |',
      ...rows.map(row => '| ' + row.join(' | ') + ' |'),
    ].join('\n');
    this.outputs.push(table);
  }
  
  async renderInteractive(component: any): Promise<any> {
    console.error('[MCP] Interactive components not supported');
    return null;
  }
  
  openPanel(title: string, content: PanelContent) {
    console.error('[MCP] Panels not supported, rendering inline');
    if (content.type === 'markdown') {
      this.renderMarkdown(`## ${title}\n\n${content.content}`);
    }
    return {
      id: 'panel_1',
      update: () => {},
      close: () => {},
    };
  }
  
  async showForm(schema: FormSchema): Promise<FormData> {
    console.error('[MCP] Forms should be handled via function parameters');
    return {};
  }
  
  progress(percent: number, message?: string): void {
    console.error(`[MCP] Progress: ${percent}% ${message || ''}`);
  }
  
  clear(): void {
    this.outputs = [];
  }
}

class McpStorageBackend implements StorageBackend {
  private memory: Map<string, any> = new Map();
  readonly isPersistent = false;
  
  async get<T>(key: string): Promise<T | undefined> {
    return this.memory.get(key);
  }
  
  async set<T>(key: string, value: T): Promise<void> {
    this.memory.set(key, value);
  }
  
  async delete(key: string): Promise<void> {
    this.memory.delete(key);
  }
  
  async keys(): Promise<string[]> {
    return Array.from(this.memory.keys());
  }
  
  async clear(): Promise<void> {
    this.memory.clear();
  }
}
