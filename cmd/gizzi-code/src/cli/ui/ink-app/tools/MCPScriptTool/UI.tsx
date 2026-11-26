import * as React from 'react';
import { MessageResponse } from '../../components/MessageResponse';
import { Box, Text } from '../../ink';

// Row for the `mcp_script` tool (PLAN §3 `metadata.mcpScript`). Live session metadata is on
// `part.state.metadata.mcpScript`; callers pass that object here.
export interface McpScriptMeta {
  lang?: 'ts';
  script: string;
  servers: string[];
  calls: { server: string; tool: string; ms: number; ok: boolean; error?: string }[];
  durationMs: number;
  result?: unknown;
}

const MAX_SCRIPT_LINES = 15;

export function summaryLine(m: McpScriptMeta): string {
  const secs = (m.durationMs / 1000).toFixed(1);
  const n = m.servers.length;
  const c = m.calls.length;
  return `Ran a script · ${n} server${n === 1 ? '' : 's'} · ${c} call${c === 1 ? '' : 's'} · ${secs}s`;
}

export function scriptLines(script: string): string[] {
  const lines = script.split('\n');
  return lines.length > MAX_SCRIPT_LINES
    ? [...lines.slice(0, MAX_SCRIPT_LINES), `… +${lines.length - MAX_SCRIPT_LINES} more lines`]
    : lines;
}

export function renderToolResultMessage(meta: McpScriptMeta | undefined, { verbose }: { verbose: boolean }): React.ReactNode {
  if (!meta) return null;
  if (!verbose) {
    return <MessageResponse height={1}>
        <Text>{summaryLine(meta)}</Text>
      </MessageResponse>;
  }
  const result = meta.result === undefined ? '' : typeof meta.result === 'string' ? meta.result : JSON.stringify(meta.result, null, 2);
  return <MessageResponse>
      <Box flexDirection="column">
        <Text>{summaryLine(meta)}</Text>
        {scriptLines(meta.script).map((l, i) => <Text key={`s${i}`} dimColor>{l}</Text>)}
        {meta.calls.map((c, i) => <Text key={`c${i}`}>
            <Text color={c.ok ? 'success' : 'error'}>{c.ok ? '✓' : '✗'}</Text> {c.server}.{c.tool} <Text dimColor>{c.ms}ms{c.error ? ` · ${c.error}` : ''}</Text>
          </Text>)}
        {result && <Text>{result}</Text>}
      </Box>
    </MessageResponse>;
}
