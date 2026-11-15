import { feature } from 'bun:bundle';
import figures from 'figures';
import * as React from 'react';
import type { z } from 'zod/v4';
import { ProgressBar } from '../../components/design-system/ProgressBar';
import { MessageResponse } from '../../components/MessageResponse';
import { linkifyUrlsInText, OutputLine } from '../../components/shell/OutputLine';
import { stringWidth } from '../../ink/stringWidth';
import { Ansi, Box, Text } from '../../ink';
import type { ToolProgressData } from '../../Tool';
import type { ProgressMessage } from '../../types/message';
// TODO(types): ink-app/types/tools.ts is a dead stub, and the canonical
// src/types/tools.ts MCPProgress (type: 'mcp', serverName/operation) does not
// match the payload shape consumed here (progress/total/progressMessage).
// Mirror the consumed shape locally (sibling TODO(types) pattern, cf.
// WebSearchTool).
import { formatNumber } from '../../utils/format';
import { createHyperlink } from '../../utils/hyperlink';
import { getContentSizeEstimate, type MCPToolResult } from '../../utils/mcpValidation';
import { appViewFor } from '../../services/mcp/appView';
import { jsonParse, jsonStringify } from '../../utils/slowOperations';
import type { inputSchema } from './MCPTool';
import { formatMessagesResult } from './messagesResult';

// Threshold for displaying warning about large MCP responses
const MCP_OUTPUT_WARNING_THRESHOLD_TOKENS = 10_000;

// In non-verbose mode, truncate individual input values to keep the header
// compact. Matches BashTool's philosophy of showing enough to identify the
// call without dumping the entire payload inline.
const MAX_INPUT_VALUE_CHARS = 80;

// Max number of top-level keys before we fall back to raw JSON display.
// Beyond this a flat k:v list is more noise than help.
const MAX_FLAT_JSON_KEYS = 12;

// Don't attempt flat-object parsing for large blobs.
const MAX_FLAT_JSON_CHARS = 5_000;

// Don't attempt to parse JSON blobs larger than this (perf safety).
const MAX_JSON_PARSE_CHARS = 200_000;

interface MCPProgress {
  type?: string;
  progress?: number;
  total?: number;
  progressMessage?: string;
}

// A string value is "dominant text payload" if it has newlines or is
// long enough that inline display would be worse than unwrapping.
const UNWRAP_MIN_STRING_LEN = 200;
export function renderToolUseMessage(input: z.infer<ReturnType<typeof inputSchema>>, {
  verbose
}: {
  verbose: boolean;
}): React.ReactNode {
  if (Object.keys(input).length === 0) {
    return '';
  }
  return Object.entries(input).map(([key, value]) => {
    let rendered = jsonStringify(value);
    if (feature('MCP_RICH_OUTPUT') && !verbose && rendered.length > MAX_INPUT_VALUE_CHARS) {
      rendered = rendered.slice(0, MAX_INPUT_VALUE_CHARS).trimEnd() + '…';
    }
    return `${key}: ${rendered}`;
  }).join(', ');
}
export function renderToolUseProgressMessage(progressMessagesForMessage: ProgressMessage<MCPProgress>[]): React.ReactNode {
  const lastProgress = progressMessagesForMessage.at(-1);
  if (!lastProgress?.data) {
    return <MessageResponse height={1}>
        <Text dimColor>Running…</Text>
      </MessageResponse>;
  }
  const {
    progress,
    total,
    progressMessage
  } = lastProgress.data;
  if (progress === undefined) {
    return <MessageResponse height={1}>
        <Text dimColor>Running…</Text>
      </MessageResponse>;
  }
  if (total !== undefined && total > 0) {
    const ratio = Math.min(1, Math.max(0, progress / total));
    const percentage = Math.round(ratio * 100);
    return <MessageResponse>
        <Box flexDirection="column">
          {progressMessage && <Text dimColor>{progressMessage}</Text>}
          <Box flexDirection="row" gap={1}>
            <ProgressBar ratio={ratio} width={20} />
            <Text dimColor>{percentage}%</Text>
          </Box>
        </Box>
      </MessageResponse>;
  }
  return <MessageResponse height={1}>
      <Text dimColor>{progressMessage ?? `Processing… ${progress}`}</Text>
    </MessageResponse>;
}
export function renderToolResultMessage(output: string | MCPToolResult, _progressMessagesForMessage: ProgressMessage<ToolProgressData>[], {
  verbose,
  input
}: {
  verbose: boolean;
  input?: unknown;
}): React.ReactNode {
  const mcpOutput = output as MCPToolResult;
  if (!verbose) {
    const slackSend = trySlackSendCompact(mcpOutput, input);
    if (slackSend !== null) {
      return <MessageResponse height={1}>
          <Text>
            Sent a message to{' '}
            <Ansi>{createHyperlink(slackSend.url, slackSend.channel)}</Ansi>
          </Text>
        </MessageResponse>;
    }
  }
  const messagesView = formatMessagesResult(mcpOutput, input);
  if (messagesView !== null) {
    return <MessageResponse>
        <Box flexDirection="column">
          {messagesView.lines.map((line, i) => <Text key={i} dimColor={i > 0 && messagesView.kind === 'search'}>{line}</Text>)}
        </Box>
      </MessageResponse>;
  }
  const estimatedTokens = getContentSizeEstimate(mcpOutput);
  const showWarning = estimatedTokens > MCP_OUTPUT_WARNING_THRESHOLD_TOKENS;
  const warningMessage = showWarning ? `${figures.warning} Large MCP response (~${formatNumber(estimatedTokens)} tokens), this can fill up context quickly` : null;
  let contentElement: React.ReactNode;
  if (Array.isArray(mcpOutput)) {
    const contentBlocks = mcpOutput.map((item, i) => {
      if (item.type === 'image') {
        return <Box key={i} justifyContent="space-between" overflowX="hidden" width="100%">
            <MessageResponse height={1}>
              <Text>[Image]</Text>
            </MessageResponse>
          </Box>;
      }
      // For text blocks and any other block types, extract text if available
      const textContent = item.type === 'text' && 'text' in item && item.text !== null && item.text !== undefined ? String(item.text) : '';
      return feature('MCP_RICH_OUTPUT') ? <MCPTextOutput key={i} content={textContent} verbose={verbose} /> : <OutputLine key={i} content={textContent} verbose={verbose} />;
    });

    // Wrap array content in a column layout
    contentElement = <Box flexDirection="column" width="100%">
        {contentBlocks}
      </Box>;
  } else if (!mcpOutput) {
    contentElement = <Box justifyContent="space-between" overflowX="hidden" width="100%">
        <MessageResponse height={1}>
          <Text dimColor>(No content)</Text>
        </MessageResponse>
      </Box>;
  } else {
    contentElement = feature('MCP_RICH_OUTPUT') ? <MCPTextOutput content={mcpOutput} verbose={verbose} /> : <OutputLine content={mcpOutput} verbose={verbose} />;
  }
  if (warningMessage) {
    return <Box flexDirection="column">
        <MessageResponse height={1}>
          <Text color="warning">{warningMessage}</Text>
        </MessageResponse>
        {contentElement}
      </Box>;
  }
  return contentElement;
}

/**
 * Render MCP text output. Tries three strategies in order:
 * 1. If JSON wraps a single dominant text payload (e.g. slack's
 *    {"messages":"line1\nline2..."}), unwrap and let OutputLine truncate.
 * 2. If JSON is a small flat-ish object, render as aligned key: value.
 * 3. Otherwise fall through to OutputLine (pretty-print + truncate).
 */
function MCPTextOutput(t0) {
  const {
    content,
    verbose
  } = t0;
  let t1;
  t1 = Symbol.for("react.early_return_sentinel");
  bb0: {
      const unwrapped = tryUnwrapTextPayload(content);
      if (unwrapped !== null) {
        const t2 = unwrapped.extras.length > 0 && <Text dimColor={true}>{unwrapped.extras.map(_temp).join(" \xB7 ")}</Text>;
        const t3 = <OutputLine content={unwrapped.body} verbose={verbose} linkifyUrls={true} />;

        const t4 = <MessageResponse><Box flexDirection="column">{t2}{t3}</Box></MessageResponse>;

        t1 = t4;
        break bb0;
      }
    }

  if (t1 !== Symbol.for("react.early_return_sentinel")) {
    return t1;
  }
  let t2;
  t2 = Symbol.for("react.early_return_sentinel");
  bb1: {
      const flat = tryFlattenJson(content);
      if (flat !== null) {
        const maxKeyWidth = Math.max(...flat.map(_temp2));
        const t3 = (t4, i) => {
            const [key, value] = t4;
            return <Text key={i}><Text dimColor={true}>{key.padEnd(maxKeyWidth)}: </Text><Ansi>{linkifyUrlsInText(value)}</Ansi></Text>;
          };

        const t4 = <Box flexDirection="column">{flat.map(t3)}</Box>;
        const t5 = <MessageResponse>{t4}</MessageResponse>;

        t2 = t5;
        break bb1;
      }
    }

  if (t2 !== Symbol.for("react.early_return_sentinel")) {
    return t2;
  }
  const t3 = <OutputLine content={content} verbose={verbose} linkifyUrls={true} />;

  return t3;
}

/**
 * Parse content as a JSON object and return its entries. Null if content
 * doesn't parse, isn't an object, is too large, or has 0/too-many keys.
 */
function _temp2(t0) {
  const [k_0] = t0;
  return stringWidth(k_0);
}
function _temp(t0) {
  const [k, v] = t0;
  return `${k}: ${v}`;
}
function parseJsonEntries(content: string, {
  maxChars,
  maxKeys
}: {
  maxChars: number;
  maxKeys: number;
}): [string, unknown][] | null {
  const trimmed = content.trim();
  if (trimmed.length === 0 || trimmed.length > maxChars || trimmed[0] !== '{') {
    return null;
  }
  let parsed: unknown;
  try {
    parsed = jsonParse(trimmed);
  } catch {
    return null;
  }
  if (parsed === null || typeof parsed !== 'object' || Array.isArray(parsed)) {
    return null;
  }
  const entries = Object.entries(parsed);
  if (entries.length === 0 || entries.length > maxKeys) {
    return null;
  }
  return entries;
}

/**
 * If content parses as a JSON object where every value is a scalar or a
 * small nested object, flatten it to [key, displayValue] pairs. Nested
 * objects get one-line JSON. Returns null if content doesn't qualify.
 */
export function tryFlattenJson(content: string): [string, string][] | null {
  const entries = parseJsonEntries(content, {
    maxChars: MAX_FLAT_JSON_CHARS,
    maxKeys: MAX_FLAT_JSON_KEYS
  });
  if (entries === null) return null;
  const result: [string, string][] = [];
  for (const [key, value] of entries) {
    if (typeof value === 'string') {
      result.push([key, value]);
    } else if (value === null || typeof value === 'number' || typeof value === 'boolean') {
      result.push([key, String(value)]);
    } else if (typeof value === 'object') {
      const compact = jsonStringify(value);
      if (compact.length > 120) return null;
      result.push([key, compact]);
    } else {
      return null;
    }
  }
  return result;
}

/**
 * If content is a JSON object where one key holds a dominant string payload
 * (multiline or long) and all siblings are small scalars, unwrap it. This
 * handles the common MCP pattern of {"messages":"line1\nline2..."} where
 * pretty-printing keeps \n escaped but we want real line breaks + truncation.
 */
export function tryUnwrapTextPayload(content: string): {
  body: string;
  extras: [string, string][];
} | null {
  const entries = parseJsonEntries(content, {
    maxChars: MAX_JSON_PARSE_CHARS,
    maxKeys: 4
  });
  if (entries === null) return null;
  // Find the one dominant string payload. Trim first: a trailing \n on a
  // short sibling (e.g. pagination hints) shouldn't make it "dominant".
  let body: string | null = null;
  const extras: [string, string][] = [];
  for (const [key, value] of entries) {
    if (typeof value === 'string') {
      const t = value.trimEnd();
      const isDominant = t.length > UNWRAP_MIN_STRING_LEN || t.includes('\n') && t.length > 50;
      if (isDominant) {
        if (body !== null) return null; // two big strings — ambiguous
        body = t;
        continue;
      }
      if (t.length > 150) return null;
      extras.push([key, t.replace(/\s+/g, ' ')]);
    } else if (value === null || typeof value === 'number' || typeof value === 'boolean') {
      extras.push([key, String(value)]);
    } else {
      return null; // nested object/array — use flat or pretty-print path
    }
  }
  if (body === null) return null;
  return {
    body,
    extras
  };
}
const SLACK_ARCHIVES_RE = /^https:\/\/[a-z0-9-]+\.slack\.com\/archives\/([A-Z0-9]+)\/p\d+$/;

/**
 * Detect a Slack send-message result and return a compact {channel, url} pair.
 * Matches both hosted (claude.ai Slack) and community MCP server shapes —
 * both return `message_link` in the result. The channel label prefers the
 * tool input (may be a name like "#foo" or an ID like "C09EVDAN1NK") and
 * falls back to the ID parsed from the archives URL.
 */
export function trySlackSendCompact(output: string | MCPToolResult, input: unknown): {
  channel: string;
  url: string;
} | null {
  let text: unknown = output;
  if (Array.isArray(output)) {
    const block = output.find(b => b.type === 'text');
    text = block && 'text' in block ? block.text : undefined;
  }
  if (typeof text !== 'string' || !text.includes('"message_link"')) {
    return null;
  }
  const entries = parseJsonEntries(text, {
    maxChars: 2000,
    maxKeys: 6
  });
  const url = entries?.find(([k]) => k === 'message_link')?.[1];
  if (typeof url !== 'string') return null;
  const m = SLACK_ARCHIVES_RE.exec(url);
  if (!m) return null;
  const inp = input as {
    channel_id?: unknown;
    channel?: unknown;
  } | undefined;
  const raw = inp?.channel_id ?? inp?.channel ?? m[1];
  const label = typeof raw === 'string' && raw ? raw : 'slack';
  return {
    channel: label.startsWith('#') ? label : `#${label}`,
    url
  };
}

/**
 * MCP Apps: the normal result, then one line that opens the tool's ui://
 * app in the browser (services/mcp/appView.ts writes the host page).
 */
export function renderToolResultWithAppView(output: string | MCPToolResult, progressMessagesForMessage: ProgressMessage<ToolProgressData>[], options: {
  verbose: boolean;
  input?: unknown;
}): React.ReactNode {
  const base = renderToolResultMessage(output, progressMessagesForMessage, options);
  const view = appViewFor(output);
  if (!view) return base;
  return <Box flexDirection="column" width="100%">
      {base}
      <MessageResponse height={1}>
        <Text>
          <Text color="suggestion">⧉ MCP App</Text> · <Ansi>{createHyperlink(view.url, 'View in browser')}</Ansi>
          <Text dimColor> {view.file}</Text>
        </Text>
      </MessageResponse>
    </Box>;
}
