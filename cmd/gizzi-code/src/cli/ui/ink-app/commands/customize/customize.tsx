import figures from 'figures';
import * as React from 'react';
import { useEffect, useMemo, useState } from 'react';
import { Pane } from '../../components/design-system/Pane';
import { useRegisterOverlay } from '../../context/overlayContext.js';
import { Box, Text, useInput } from '../../ink';
import { addMcpConfig } from '../../services/mcp/config.js';
import type { LocalJSXCommandCall } from '../../types/command.js';
import { addedIds, addsWhat, type CatalogItem, mcpEntry, SECTIONS, tuiMcpConfig, writeGizziCommand, writeGizziMcp } from './catalog';

const WINDOW = 14;

type Row = { kind: 'shelf'; title: string } | { kind: 'item'; item: CatalogItem };

async function platform() {
  return import('@/runtime/bots/platform-api');
}

/** Installed catalog skills, from the same local route Desktop reads. Empty when the local API isn't running. */
async function installedSkills(): Promise<string[]> {
  try {
    const { platformFetch } = await platform();
    const res = await platformFetch('GET', '/api/v1/skills/catalog/installed', undefined, { timeoutMs: 5000 });
    if (!res.ok) return [];
    const body = (await res.json()) as { installed?: string[] };
    return Array.isArray(body.installed) ? body.installed : [];
  } catch {
    return [];
  }
}

/** First lines of a catalog skill's SKILL.md (Desktop's item page reads the same route). */
async function skillPreview(id: string): Promise<string[] | null> {
  try {
    const { platformFetch } = await platform();
    const res = await platformFetch('GET', `/api/v1/skills/catalog/${encodeURIComponent(id)}/files`, undefined, { timeoutMs: 15000 });
    if (!res.ok) return null;
    const body = (await res.json()) as { files?: Array<{ path: string; text?: string }> };
    const text = body.files?.find(f => f.path === 'SKILL.md')?.text;
    if (!text) return null;
    return text.replace(/^---[\s\S]*?---\s*/, '').split('\n').filter(l => l.trim()).slice(0, 6);
  } catch {
    return null;
  }
}

/** Add, exactly as Desktop's Add does. Returns a one-line result. */
export async function addItem(item: CatalogItem): Promise<string> {
  const add = item.add;
  if ('skill' in add) {
    const { platformFetch, PlatformSignedOutError } = await platform();
    try {
      const res = await platformFetch('POST', '/api/v1/skills/catalog/install', { id: add.skill }, { timeoutMs: 60000 });
      const body = (await res.json().catch(() => ({}))) as { path?: string; message?: string };
      if (!res.ok) return `Couldn't add ${item.name}: ${body.message ?? `HTTP ${res.status}`}`;
      return `Added ${item.name} → ${body.path ?? '~/.agents/skills'}`;
    } catch (err) {
      if (err instanceof PlatformSignedOutError) return `Couldn't add ${item.name}: ${err.message}`;
      return `Couldn't add ${item.name}: Allternit's local API isn't running (open Allternit Desktop, or run gizzi serve with allternit-api).`;
    }
  }
  if ('mcp' in add) {
    const entry = mcpEntry(add.mcp);
    const file = await writeGizziMcp(add.mcp.key, entry);
    // This terminal reads its own MCP config; add it there too so /mcp shows it.
    const tui = tuiMcpConfig(entry);
    if (tui) await addMcpConfig(add.mcp.key, tui, 'user').catch(() => {});
    return `Added ${item.name} to ${file}${item.requires ? ` (needs ${item.requires})` : ''}`;
  }
  const file = await writeGizziCommand(add.command.name, item.description, add.command.template);
  return `Added /${add.command.name} → ${file}`;
}

function Customize({ onDone }: { onDone: (result?: string) => void }): React.ReactNode {
  const [sectionIdx, setSectionIdx] = useState(0);
  const [focus, setFocus] = useState(0);
  const [page, setPage] = useState<CatalogItem | null>(null);
  const [preview, setPreview] = useState<string[] | null>(null);
  const [added, setAdded] = useState<Set<string>>(new Set());
  const [status, setStatus] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const section = SECTIONS[sectionIdx]!;
  const rows = useMemo<Row[]>(() => section.shelves.flatMap(s => [{ kind: 'shelf' as const, title: s.title }, ...s.items.map(item => ({ kind: 'item' as const, item }))]), [section]);
  const items = rows.filter((r): r is Extract<Row, { kind: 'item' }> => r.kind === 'item');
  const focused = items[Math.min(focus, items.length - 1)]?.item;

  useEffect(() => {
    void installedSkills().then(addedIds).then(setAdded).catch(() => {});
  }, []);
  useEffect(() => {
    setPreview(null);
    if (page && 'skill' in page.add) void skillPreview(page.add.skill).then(setPreview);
  }, [page]);

  useRegisterOverlay('customize', true);

  const add = (item: CatalogItem) => {
    if (busy) return;
    if (added.has(item.id)) {
      setStatus(`${item.name} is already added.`);
      return;
    }
    setBusy(true);
    setStatus(`Adding ${item.name}…`);
    addItem(item).then(message => {
      setStatus(message);
      if (message.startsWith('Added')) setAdded(prev => new Set(prev).add(item.id));
    }, (err: Error) => setStatus(`Couldn't add ${item.name}: ${err.message}`)).finally(() => setBusy(false));
  };

  useInput((input, key) => {
    if (page) {
      if (key.escape || key.leftArrow) setPage(null);
      else if (input === 'a') add(page);
      return;
    }
    if (key.escape || input === 'q') return onDone(undefined);
    if (key.upArrow) setFocus(f => Math.max(0, f - 1));
    else if (key.downArrow) setFocus(f => Math.min(items.length - 1, f + 1));
    else if (key.tab || key.rightArrow || key.leftArrow) {
      const dir = key.leftArrow || key.shift ? -1 : 1;
      setSectionIdx(i => (i + dir + SECTIONS.length) % SECTIONS.length);
      setFocus(0);
      setStatus(null);
    } else if (key.return && focused) setPage(focused);
    else if (input === 'a' && focused) add(focused);
  });

  const tabs = <Box gap={2}>{SECTIONS.map((s, i) => <Text key={s.id} bold={i === sectionIdx} color={i === sectionIdx ? 'suggestion' : undefined} dimColor={i !== sectionIdx}>{s.title}</Text>)}</Box>;
  const statusLine = status ? <Text color={status.startsWith("Couldn't") ? 'error' : status.startsWith('Added') ? 'success' : undefined}>{status}</Text> : null;

  if (page) {
    const isAdded = added.has(page.id);
    return <Pane color="permission">
        <Box flexDirection="column" gap={1}>
          <Box flexDirection="column">
            <Text bold>{page.name}{isAdded ? <Text color="success"> {figures.tick} Added</Text> : null}</Text>
            <Text dimColor>{[page.kind, page.author, page.version ? `v${page.version}` : page.category].filter(Boolean).join(' · ')}</Text>
          </Box>
          <Text>{page.description}</Text>
          {preview && <Box flexDirection="column"><Text dimColor bold>SKILL.md</Text>{preview.map((l, i) => <Text key={i} dimColor wrap="truncate-end">{l}</Text>)}</Box>}
          {page.requires && <Text dimColor>Needs {page.requires} on this computer.</Text>}
          {page.homepage && <Text dimColor>{page.homepage}</Text>}
          <Text dimColor>Add: {addsWhat(page)}</Text>
          {statusLine}
          <Text dimColor>{isAdded ? '' : 'a add · '}Esc back</Text>
        </Box>
      </Pane>;
  }

  const focusRow = rows.findIndex(r => r.kind === 'item' && r.item === focused);
  const start = Math.max(0, Math.min(focusRow - Math.floor(WINDOW / 2), rows.length - WINDOW));
  return <Pane color="permission">
      <Box flexDirection="column">
        <Text color="remember" bold>Customize</Text>
        <Text dimColor>What your chats and bots can use. Same catalog and Add as Allternit Desktop.</Text>
        <Box marginY={1}>{tabs}</Box>
        {rows.slice(start, start + WINDOW).map((row, i) => {
          if (row.kind === 'shelf') return <Text key={`s-${start + i}`} bold dimColor>{row.title}</Text>;
          const isFocused = row.item === focused;
          return <Box key={row.item.id} flexDirection="row">
              <Text color={isFocused ? 'suggestion' : undefined} wrap="truncate-end">{isFocused ? figures.pointer : ' '} {row.item.name}<Text dimColor> · {row.item.description}</Text></Text>
              <Box flexGrow={1} />
              <Box flexShrink={0} marginLeft={1}>{added.has(row.item.id) ? <Text color="success">{figures.tick} Added</Text> : <Text dimColor>{row.item.category ?? row.item.author}</Text>}</Box>
            </Box>;
        })}
        <Box marginTop={1} flexDirection="column">
          {statusLine}
          <Text dimColor>↑/↓ move · Tab section · Enter item page · a add · Esc close</Text>
        </Box>
      </Box>
    </Pane>;
}

export const call: LocalJSXCommandCall = async (onDone, context) => {
  if (context.options?.isNonInteractiveSession) {
    onDone('/customize is only available in the interactive TUI. Add MCP servers with `gizzi mcp add`.', { display: 'system' });
    return null;
  }
  return <Customize onDone={result => onDone(result, { display: result ? 'system' : 'skip' })} />;
};
