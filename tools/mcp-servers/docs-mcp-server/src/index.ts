#!/usr/bin/env node

import { realpathSync } from 'node:fs';
import { readdir, readFile } from 'node:fs/promises';
import { join, relative, extname } from 'node:path';
import { pathToFileURL } from 'node:url';
import { ProtocolError, ProtocolErrorCode, Server, type Tool } from '@modelcontextprotocol/server';
import { serveStdio } from '@modelcontextprotocol/server/stdio';

const SERVER_NAME = 'allternit-docs-mcp';
const SERVER_VERSION = '1.0.0';

// Resolve the docs directory relative to this server's location
const DOCS_ROOT = process.env.ALLTERNIT_DOCS_ROOT
  || join(new URL('.', import.meta.url).pathname, '..', '..', '..', '..', 'docs', 'public');

interface ToolDefinition {
  name: string;
  description: string;
  inputSchema: {
    type: 'object';
    properties: Record<string, unknown>;
    required?: string[];
  };
}

/** Fixed order: tools/list always returns these in this order. */
export const TOOLS: ToolDefinition[] = [
  {
    name: 'search_docs',
    description: 'Search Allternit platform documentation by keyword. Returns matching document titles, paths, and summaries.',
    inputSchema: {
      type: 'object',
      properties: {
        query: { type: 'string', description: 'Search query — keywords or topic to find in documentation' },
        limit: { type: 'number', description: 'Maximum number of results (default: 10)' },
      },
      required: ['query'],
    },
  },
  {
    name: 'read_doc',
    description: 'Read the full content of a specific Allternit documentation page by its path.',
    inputSchema: {
      type: 'object',
      properties: {
        path: { type: 'string', description: 'The relative path to the doc file (e.g. "tools/tool-belt.md")' },
      },
      required: ['path'],
    },
  },
  {
    name: 'list_docs',
    description: 'List all available documentation files organized by category (tools, providers, guides, ACI, etc.).',
    inputSchema: {
      type: 'object',
      properties: {
        category: { type: 'string', description: 'Filter by category folder (e.g. "tools", "providers", "guides")' },
      },
    },
  },
  {
    name: 'get_api_reference',
    description: 'Look up API endpoint documentation for Allternit REST APIs (marketplace, agents, sessions, etc.).',
    inputSchema: {
      type: 'object',
      properties: {
        endpoint: { type: 'string', description: 'The API endpoint path or name (e.g. "/v1/marketplace/capabilities")' },
      },
      required: ['endpoint'],
    },
  },
];

// --- Doc scanning utilities ---

const DOC_EXTENSIONS = new Set(['.md', '.mdx', '.txt']);

async function scanDocs(root: string): Promise<Array<{ path: string; title: string; category: string }>> {
  const results: Array<{ path: string; title: string; category: string }> = [];
  try {
    await walkDir(root, root, results);
  } catch {
    // Docs directory may not exist in all environments
  }
  return results;
}

async function walkDir(
  dir: string,
  root: string,
  results: Array<{ path: string; title: string; category: string }>,
): Promise<void> {
  let entries: import('node:fs').Dirent[];
  try {
    entries = await readdir(dir, { withFileTypes: true });
  } catch {
    return;
  }
  for (const entry of entries) {
    const fullPath = join(dir, entry.name);
    if (entry.isDirectory()) {
      if (entry.name === 'node_modules' || entry.name.startsWith('.')) continue;
      await walkDir(fullPath, root, results);
    } else if (entry.isFile() && DOC_EXTENSIONS.has(extname(entry.name).toLowerCase())) {
      const relPath = relative(root, fullPath);
      const category = relPath.split('/')[0] || 'root';
      const content = await readFile(fullPath, 'utf-8');
      const titleMatch = content.match(/^#\s+(.+)$/m);
      const title = titleMatch?.[1]?.trim() || entry.name.replace(extname(entry.name), '');
      results.push({ path: relPath, title, category });
    }
  }
}

async function readDocContent(root: string, docPath: string): Promise<string | null> {
  const normalizedPath = docPath.replace(/\\/g, '/').replace(/^\/+/, '');
  if (normalizedPath.includes('..')) return null;
  const fullPath = join(root, normalizedPath);
  try {
    return await readFile(fullPath, 'utf-8');
  } catch {
    return null;
  }
}

// --- Tool execution ---

export async function executeTool(name: string, args: Record<string, unknown>): Promise<{ content: Array<{ type: 'text'; text: string }> }> {
  switch (name) {
    case 'search_docs': {
      const query = String(args.query || '').toLowerCase();
      const limit = Number(args.limit) || 10;
      const docs = await scanDocs(DOCS_ROOT);
      const matches = docs.filter(
        (d) => d.title.toLowerCase().includes(query) || d.path.toLowerCase().includes(query) || d.category.toLowerCase().includes(query),
      ).slice(0, limit);
      if (matches.length === 0) {
        return { content: [{ type: 'text', text: `No documentation found matching "${query}".` }] };
      }
      const formatted = matches.map((m) => `- **${m.title}** (${m.category})\n  Path: \`${m.path}\``).join('\n');
      return { content: [{ type: 'text', text: `Found ${matches.length} result(s):\n\n${formatted}` }] };
    }
    case 'read_doc': {
      const docPath = String(args.path || '');
      const content = await readDocContent(DOCS_ROOT, docPath);
      if (!content) {
        return { content: [{ type: 'text', text: `Document not found: ${docPath}` }] };
      }
      return { content: [{ type: 'text', text: content }] };
    }
    case 'list_docs': {
      const categoryFilter = args.category ? String(args.category).toLowerCase() : null;
      const docs = await scanDocs(DOCS_ROOT);
      const filtered = categoryFilter ? docs.filter((d) => d.category.toLowerCase() === categoryFilter) : docs;
      const grouped = new Map<string, Array<{ path: string; title: string }>>();
      for (const doc of filtered) {
        const list = grouped.get(doc.category) || [];
        list.push({ path: doc.path, title: doc.title });
        grouped.set(doc.category, list);
      }
      const sections: string[] = [];
      for (const [cat, items] of grouped) {
        sections.push(`## ${cat}\n${items.map((i) => `- ${i.title} — \`${i.path}\``).join('\n')}`);
      }
      const text = sections.length > 0 ? sections.join('\n\n') : 'No documentation files found.';
      return { content: [{ type: 'text', text }] };
    }
    case 'get_api_reference': {
      const endpoint = String(args.endpoint || '').toLowerCase();
      const docs = await scanDocs(DOCS_ROOT);
      const matches = docs.filter(
        (d) => d.path.toLowerCase().includes('api') || d.path.toLowerCase().includes('route') || d.path.toLowerCase().includes('endpoint'),
      ).filter((d) => d.path.toLowerCase().includes(endpoint) || d.title.toLowerCase().includes(endpoint));
      if (matches.length === 0) {
        return { content: [{ type: 'text', text: `No API reference found for "${endpoint}". Try list_docs to see available categories.` }] };
      }
      const results: string[] = [];
      for (const match of matches.slice(0, 3)) {
        const content = await readDocContent(DOCS_ROOT, match.path);
        results.push(`## ${match.title}\nPath: \`${match.path}\`\n\n${content?.slice(0, 2000) || '(empty)'}`);
      }
      return { content: [{ type: 'text', text: results.join('\n\n---\n\n') }] };
    }
    default:
      return { content: [{ type: 'text', text: `Unknown tool: ${name}` }] };
  }
}

// --- MCP server (official SDK v2) ---
// serveStdio speaks MCP 2026-07-28 (server/discover, per-request _meta) and the
// legacy initialize handshake from the same factory.

export function createDocsServer(): Server {
  const server = new Server({ name: SERVER_NAME, version: SERVER_VERSION }, { capabilities: { tools: {} } });
  server.setRequestHandler('tools/list', async () => ({ tools: TOOLS as Tool[] }));
  server.setRequestHandler('tools/call', async (request) => {
    const toolName = String(request.params.name || '');
    const toolArgs = (request.params.arguments || {}) as Record<string, unknown>;
    try {
      return await executeTool(toolName, toolArgs);
    } catch (err) {
      throw new ProtocolError(
        ProtocolErrorCode.InternalError,
        `Tool execution failed: ${err instanceof Error ? err.message : String(err)}`,
      );
    }
  });
  return server;
}

function isEntry(): boolean {
  try {
    return import.meta.url === pathToFileURL(realpathSync(process.argv[1] ?? '')).href;
  } catch {
    return false;
  }
}

if (isEntry()) {
  serveStdio(() => createDocsServer());
}
