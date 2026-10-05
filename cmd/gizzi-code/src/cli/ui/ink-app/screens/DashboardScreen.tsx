// @ts-nocheck
/**
 * DashboardScreen — full-screen agent dashboard (Grok `/dashboard` parity).
 *
 * Mounted by REPL when AppState.screen === 'dashboard'. Renders the live
 * roster of top-level sessions from the DashboardSource plus:
 * - dispatch input (spawn new top-level sessions)
 * - inline peek panel for the selected row (last response, reply box)
 * - details view (full session transcript via the real Messages renderer)
 * - search (Ctrl+/), grouping (Ctrl+G), rename (Ctrl+R), pin (Ctrl+T),
 *   reorder (Shift+↑/↓), stop/remove (x), cheatsheet (?)
 *
 * Esc ladder: cheatsheet → details → search → peek reply → peek → exit
 * (the `dashboard:exit` keybinding is gated to plain list focus so inner
 * views claim Esc first; single-char chords like q are gated the same way).
 */

import * as React from 'react';
import { useRegisterOverlay } from '../context/overlayContext';
import { useTerminalSize } from '../hooks/useTerminalSize';
import { Box, Text, useInput } from '../ink';
import { useKeybinding } from '../keybindings/useKeybinding';
import { useAppState, useSetAppState } from '../state/AppState';
import { Messages } from '../components/Messages';
import { PermissionRequest } from '../components/permissions/PermissionRequest';
import ScrollBox, { type ScrollBoxHandle } from '../ink/components/ScrollBox';
import type { DashboardRow, DashboardSource } from '../dashboard/types';

// Stable empty props for the read-only transcript renderer. Fresh
// arrays/Sets every render would defeat Messages' React.memo and
// re-render the whole transcript on each dashboard tick.
const TRANSCRIPT_EMPTY_ARR: unknown[] = [];
const TRANSCRIPT_NO_TOOL_USES: Set<string> = new Set();

// Braille spinner frames for working rows (was a static ●).
const SPIN_FRAMES = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

const STATE_ORDER: Record<string, number> = {
  working: 0,
  'needs-input': 1,
  idle: 2,
  completed: 3,
  failed: 4,
  inactive: 5,
};

const STATE_LABEL: Record<string, string> = {
  working: 'Working',
  'needs-input': 'Awaiting input',
  idle: 'Idle',
  completed: 'Done',
  failed: 'Failed',
  inactive: 'Inactive',
};

const IDLE_CUTOFF_MS = 60 * 60 * 1000; // 1h
const FRESH_LIMIT = 8;

type GroupBy = 'none' | 'state' | 'directory';

type Item =
  | { type: 'header'; key: string; label: string; count: number; collapsed: boolean }
  | { type: 'row'; row: DashboardRow }
  | { type: 'more'; count: number };

function truncate(text: string, max: number): string {
  if (text.length <= max) return text;
  return `${text.slice(0, Math.max(0, max - 1))}…`;
}

// Ink leaves stale terminal cells behind when a line shrinks between frames;
// pad input lines to the full width so old content is always overwritten.
function padLine(text: string, width: number): string {
  return text.length >= width ? text : text + ' '.repeat(width - text.length);
}

function stateGlyph(row: DashboardRow): { glyph: string; color: string } {
  switch (row.state) {
    case 'working':
      return { glyph: '●', color: 'success' };
    case 'needs-input':
      return { glyph: '●', color: 'warning' };
    case 'failed':
      return { glyph: '●', color: 'error' };
    case 'completed':
      return { glyph: '●', color: 'inactive' };
    default:
      return { glyph: '○', color: 'inactive' };
  }
}

function baseName(dir: string): string {
  const parts = dir.split('/').filter(Boolean);
  return parts.at(-1) ?? dir;
}

function buildItems(
  rows: DashboardRow[],
  groupBy: GroupBy,
  expandedSections: Set<string>,
  hiddenCount: number,
  showAllIdle: boolean,
): Item[] {
  const visible = rows;
  let sections: { key: string; label: string; rows: DashboardRow[] }[];
  if (groupBy === 'none') {
    sections = [{ key: 'all', label: '', rows: visible }];
  } else if (groupBy === 'state') {
    const byState = new Map<string, DashboardRow[]>();
    for (const row of visible) {
      const list = byState.get(row.state) ?? [];
      list.push(row);
      byState.set(row.state, list);
    }
    sections = [...byState.entries()]
      .sort((a, b) => (STATE_ORDER[a[0]] ?? 9) - (STATE_ORDER[b[0]] ?? 9))
      .map(([state, list]) => ({
        key: state,
        label: `${STATE_LABEL[state] ?? state} (${list.length})`,
        rows: list,
      }));
  } else {
    const byDir = new Map<string, DashboardRow[]>();
    for (const row of visible) {
      const key = baseName(row.directory || '~');
      const list = byDir.get(key) ?? [];
      list.push(row);
      byDir.set(key, list);
    }
    sections = [...byDir.entries()]
      .sort((a, b) => {
        const aLive = a[1].some(r => r.state === 'working' || r.state === 'needs-input') ? 0 : 1;
        const bLive = b[1].some(r => r.state === 'working' || r.state === 'needs-input') ? 0 : 1;
        return aLive - bLive || a[0].localeCompare(b[0]);
      })
      .map(([dir, list]) => ({ key: `dir:${dir}`, label: `${dir} (${list.length})`, rows: list }));
  }

  const items: Item[] = [];
  for (const section of sections) {
    const collapsible = section.key !== 'all';
    const collapsed =
      collapsible && !expandedSections.has(section.key) && section.key === 'inactive';
    if (collapsible) {
      items.push({
        type: 'header',
        key: section.key,
        label: section.label,
        count: section.rows.length,
        collapsed,
      });
    }
    if (!collapsed) {
      for (const row of section.rows) items.push({ type: 'row', row });
    }
  }
  if (hiddenCount > 0 && !showAllIdle) {
    items.push({ type: 'more', count: hiddenCount });
  }
  return items;
}

export function DashboardScreen({
  source,
  tools,
  commands,
  permissionQueue,
  onPermissionDone,
}: {
  source: DashboardSource;
  tools?: unknown[];
  commands?: unknown[];
  /** Pending tool-permission confirms; items carry dashboardTaskId when
   *  they belong to a spawned session (absent = main session). */
  permissionQueue?: unknown[];
  /** Remove a confirm from the queue after the inline dialog answers it. */
  onPermissionDone?: (toolUseID: string) => void;
}): React.ReactNode {
  const { rows: termRows, columns } = useTerminalSize();
  const setAppState = useSetAppState();
  // Subscribe to task state so progress triggers re-renders; rows are read
  // from the source during render (same pattern as TasksDialog).
  useAppState(s => s.tasks);

  const [focus, setFocus] = React.useState<'list' | 'dispatch' | 'reply' | 'rename' | 'search'>('list');
  const [selected, setSelected] = React.useState(0);
  const [draft, setDraft] = React.useState('');
  const [replyDraft, setReplyDraft] = React.useState('');
  const [renameDraft, setRenameDraft] = React.useState('');
  const [searchQuery, setSearchQuery] = React.useState('');
  const [searchOpen, setSearchOpen] = React.useState(false);
  const [peekFor, setPeekFor] = React.useState<string | null>(null);
  const [detailsFor, setDetailsFor] = React.useState<string | null>(null);
  const detailsScrollRef = React.useRef<ScrollBoxHandle | null>(null);
  const [showCheatsheet, setShowCheatsheet] = React.useState(false);
  const [showAllIdle, setShowAllIdle] = React.useState(false);
  const [groupBy, setGroupBy] = React.useState<GroupBy>('none');
  const [expandedSections, setExpandedSections] = React.useState<Set<string>>(new Set());
  const [stopArmedAt, setStopArmedAt] = React.useState(0);

  // Lets CancelRequestHandler / global Esc handlers defer to us while open.
  useRegisterOverlay('dashboard');

  const exitDashboard = React.useCallback(() => {
    setAppState(prev => (prev.screen === 'dashboard' ? { ...prev, screen: 'prompt' } : prev));
  }, [setAppState]);

  const browsing =
    focus === 'list' && !peekFor && !searchOpen && !detailsFor && !showCheatsheet;
  useKeybinding('dashboard:exit', exitDashboard, {
    context: 'Dashboard',
    isActive: browsing,
  });

  // ---------------------------------------------------------------------
  // Data
  // ---------------------------------------------------------------------

  const allRows = source.list();

  // Animated spinner for working rows. Ticks only while something is
  // working; the interval tears down otherwise so an idle dashboard
  // renders nothing per frame.
  const hasWorkingRows = allRows.some(r => r.state === 'working');
  const [spinFrame, setSpinFrame] = React.useState(0);
  React.useEffect(() => {
    if (!hasWorkingRows) return;
    const t = setInterval(
      () => setSpinFrame(f => (f + 1) % SPIN_FRAMES.length),
      120,
    );
    return () => clearInterval(t);
  }, [hasWorkingRows]);

  // Pending permission prompt attributed to a row (undefined when none).
  const confirmForRow = React.useCallback(
    (row: DashboardRow) =>
      (permissionQueue ?? []).find(
        (item: any) => (item.dashboardTaskId ?? 'main') === row.id,
      ),
    [permissionQueue],
  );

  const filteredRows = React.useMemo(() => {
    if (!searchOpen || !searchQuery.trim()) return allRows;
    const q = searchQuery.trim().toLowerCase();
    return allRows.filter(row => {
      if (q.startsWith('a:')) return row.activityLine.toLowerCase().includes(q.slice(2));
      if (q.startsWith('s:')) return row.state.startsWith(q.slice(2));
      if (q.startsWith('#')) return row.id.startsWith(q.slice(1));
      return (
        row.title.toLowerCase().includes(q) ||
        row.directory.toLowerCase().includes(q)
      );
    });
  }, [allRows, searchOpen, searchQuery]);

  // Idle folding: working/needs-input/pinned/recent (<1h) always visible;
  // the rest fold down to the 8 freshest plus a "N more" row.
  const { visibleRows, hiddenCount } = React.useMemo(() => {
    const always: DashboardRow[] = [];
    const rest: DashboardRow[] = [];
    const now = Date.now();
    for (const row of filteredRows) {
      const isLive =
        row.state === 'working' ||
        row.state === 'needs-input' ||
        row.pinned ||
        now - row.updatedAt < IDLE_CUTOFF_MS;
      (isLive ? always : rest).push(row);
    }
    rest.sort((a, b) => b.updatedAt - a.updatedAt);
    return {
      visibleRows: showAllIdle ? filteredRows : [...always, ...rest.slice(0, FRESH_LIMIT)],
      hiddenCount: showAllIdle ? 0 : Math.max(0, rest.length - FRESH_LIMIT),
    };
  }, [filteredRows, showAllIdle]);

  const items = React.useMemo(
    () => buildItems(visibleRows, groupBy, expandedSections, hiddenCount, showAllIdle),
    [visibleRows, groupBy, expandedSections, hiddenCount, showAllIdle],
  );

  const rowItemIndexes = React.useMemo(
    () => items.map((item, i) => (item.type === 'header' ? -1 : i)).filter(i => i >= 0),
    [items],
  );
  const selPos = rowItemIndexes.length === 0 ? -1 : Math.min(selected, rowItemIndexes.length - 1);
  const selItemIndex = selPos >= 0 ? rowItemIndexes[selPos] : -1;
  const selectedItem = selItemIndex >= 0 ? items[selItemIndex] : undefined;
  const selectedRow =
    selectedItem?.type === 'row' ? (selectedItem as { row: DashboardRow }).row : undefined;

  const workingCount = allRows.filter(r => r.state === 'working').length;
  const awaitingCount = allRows.filter(r => r.state === 'needs-input').length;

  // ---------------------------------------------------------------------
  // Actions
  // ---------------------------------------------------------------------

  const dispatch = React.useCallback((text: string) => {
    const trimmed = text.trim();
    if (!trimmed) return;
    source.dispatch(trimmed);
  }, [source]);

  const handleStopOrRemove = React.useCallback(() => {
    if (!selectedRow) return;
    const now = Date.now();
    if (selectedRow.state === 'working' || selectedRow.state === 'needs-input') {
      source.stop(selectedRow.id);
      setStopArmedAt(now);
      return;
    }
    if (now - stopArmedAt < 2000) {
      source.remove(selectedRow.id);
      setStopArmedAt(0);
      setPeekFor(p => (p === selectedRow.id ? null : p));
      if (detailsFor === selectedRow.id) setDetailsFor(null);
      setSelected(s => Math.max(0, s - 1));
    } else {
      source.stop(selectedRow.id);
      setStopArmedAt(now);
    }
  }, [selectedRow, stopArmedAt, source, detailsFor]);

  const openPeek = React.useCallback(
    (id: string) => {
      setPeekFor(id);
      setFocus('reply');
      setReplyDraft('');
    },
    [],
  );

  const detailsTranscript =
    detailsFor && source.transcript ? source.transcript(detailsFor) : [];

  // ---------------------------------------------------------------------
  // Keys
  // ---------------------------------------------------------------------

  useInput((input, key) => {
    // ---- cheatsheet (modal-ish) ----
    if (showCheatsheet) {
      if (key.escape || input === '?') setShowCheatsheet(false);
      return;
    }

    // ---- details view ----
    if (detailsFor) {
      const scroller = detailsScrollRef.current;
      const page = Math.max(5, termRows - 10);
      if (key.escape) {
        setDetailsFor(null);
      } else if (key.upArrow || input === 'k') {
        scroller?.scrollBy(-3);
      } else if (key.downArrow || input === 'j') {
        scroller?.scrollBy(3);
      } else if ((key.ctrl && input === 'u') || key.pageUp) {
        scroller?.scrollBy(-page);
      } else if ((key.ctrl && input === 'd') || key.pageDown) {
        scroller?.scrollBy(page);
      } else if (input === 'g' && !key.ctrl && !key.meta) {
        scroller?.scrollTo(0);
      } else if (input === 'G' || key.end) {
        scroller?.scrollToBottom();
      }
      return;
    }

    // ---- text inputs ----
    if (focus === 'dispatch' || focus === 'reply' || focus === 'rename' || focus === 'search') {
      if (key.escape) {
        if (focus === 'dispatch') {
          setDraft('');
          setFocus('list');
        } else if (focus === 'reply') {
          setReplyDraft('');
          setFocus('list');
        } else if (focus === 'rename') {
          setRenameDraft('');
          setFocus('list');
        } else {
          setSearchQuery('');
          setSearchOpen(false);
          setFocus('list');
        }
        return;
      }
      if (key.return) {
        if (focus === 'dispatch') {
          dispatch(draft);
          setDraft('');
        } else if (focus === 'reply') {
          const trimmed = replyDraft.trim();
          if (trimmed && peekFor) source.reply(peekFor, trimmed);
          setReplyDraft('');
        } else if (focus === 'rename') {
          const trimmed = renameDraft.trim();
          if (trimmed && selectedRow) source.rename(selectedRow.id, trimmed);
          setRenameDraft('');
          setFocus('list');
        } else {
          setFocus('list'); // keep filter applied
        }
        return;
      }
      if (focus === 'dispatch' && key.ctrl && input === 's') {
        dispatch(draft);
        setDraft('');
        setFocus('list');
        return;
      }
      if (focus === 'reply' && key.ctrl && input === 's') {
        const trimmed = replyDraft.trim();
        if (trimmed && peekFor) source.reply(peekFor, trimmed);
        setReplyDraft('');
        setFocus('list');
        return;
      }
      if (key.backspace || key.delete) {
        if (focus === 'dispatch') setDraft(d => d.slice(0, -1));
        else if (focus === 'reply') setReplyDraft(d => d.slice(0, -1));
        else if (focus === 'rename') setRenameDraft(d => d.slice(0, -1));
        else setSearchQuery(d => d.slice(0, -1));
        return;
      }
      if (key.upArrow && focus === 'search') {
        setSelected(s => Math.max(0, s - 1));
        return;
      }
      if (key.downArrow && focus === 'search') {
        setSelected(s => Math.min(Math.max(0, rowItemIndexes.length - 1), s + 1));
        return;
      }
      if (key.ctrl || key.meta) return;
      if (input) {
        if (focus === 'dispatch') setDraft(d => d + input);
        else if (focus === 'reply') setReplyDraft(d => d + input);
        else if (focus === 'rename') setRenameDraft(d => d + input);
        else setSearchQuery(d => d + input);
      }
      return;
    }

    // ---- list focus ----
    if (key.escape) {
      if (searchOpen) {
        setSearchQuery('');
        setSearchOpen(false);
      } else if (peekFor) {
        setPeekFor(null);
      }
      // otherwise dashboard:exit (isActive while browsing) handles it
      return;
    }
    if (input === '?') {
      setShowCheatsheet(true);
      return;
    }
    if (input === 'f' && !key.ctrl && !key.meta) {
      // The Allternit Factory floor.
      setAppState(prev => (prev.screen === 'dashboard' ? { ...prev, screen: 'factory' } : prev));
      return;
    }
    if (input === 'v' && !key.ctrl && !key.meta && selectedRow) {
      setDetailsFor(selectedRow.id);
      return;
    }
    if (key.return) {
      if (selectedItem?.type === 'more') {
        setShowAllIdle(true);
        return;
      }
      if (selectedRow) {
        if (peekFor === selectedRow.id) {
          setFocus('reply');
        } else {
          openPeek(selectedRow.id);
        }
      }
      return;
    }
    if (key.upArrow) {
      if (key.shift) {
        if (selectedRow && source.move) source.move(selectedRow.id, -1);
      } else {
        setSelected(s => Math.max(0, s - 1));
      }
      return;
    }
    if (key.downArrow) {
      if (key.shift) {
        if (selectedRow && source.move) source.move(selectedRow.id, 1);
      } else {
        setSelected(s => Math.min(Math.max(0, rowItemIndexes.length - 1), s + 1));
      }
      return;
    }
    if (key.tab) {
      setFocus('dispatch');
      return;
    }
    if ((input === '/' && !key.ctrl && !key.meta) || (key.ctrl && input === '/')) {
      setSearchOpen(true);
      setSearchQuery('');
      setFocus('search');
      return;
    }
    if (key.ctrl && input === 'g') {
      setGroupBy(g => (g === 'none' ? 'state' : g === 'state' ? 'directory' : 'none'));
      setSelected(0);
      return;
    }
    if (key.ctrl && input === 's') {
      if (selectedRow && peekFor !== selectedRow.id) setPeekFor(selectedRow.id);
      return;
    }
    if ((input === 'x' && !key.ctrl && !key.meta) || (key.ctrl && input === 'x')) {
      handleStopOrRemove();
      return;
    }
    if ((input === 'p' && !key.ctrl && !key.meta) || (key.ctrl && input === 't')) {
      if (selectedRow) source.setPinned(selectedRow.id, !selectedRow.pinned);
      return;
    }
    if ((input === 'r' && !key.ctrl && !key.meta) || (key.ctrl && input === 'r')) {
      if (selectedRow) {
        setRenameDraft(selectedRow.title);
        setFocus('rename');
      }
      return;
    }
    if (input === ' ' && !key.ctrl && !key.meta) {
      // toggle section collapse (header rows are not selectable, so this
      // applies to the section containing the selected row)
      if (selectedRow && groupBy !== 'none') {
        const sectionKey = groupBy === 'state' ? selectedRow.state : `dir:${baseName(selectedRow.directory || '~')}`;
        setExpandedSections(prev => {
          const next = new Set(prev);
          if (next.has(sectionKey)) next.delete(sectionKey);
          else next.add(sectionKey);
          return next;
        });
      }
      return;
    }
  });

  // ---------------------------------------------------------------------
  // Render helpers
  // ---------------------------------------------------------------------

  const titleWidth = Math.max(10, Math.floor(columns * 0.55));
  const activityWidth = Math.max(10, columns - titleWidth - 8);
  const peek = peekFor ? source.peek(peekFor) : undefined;

  const renderRow = (item: { type: 'row'; row: DashboardRow }, itemIndex: number) => {
    const row = item.row;
    const { glyph, color } = stateGlyph(row);
    const shownGlyph = row.state === 'working' ? SPIN_FRAMES[spinFrame] : glyph;
    const isSelected = itemIndex === selItemIndex;
    const isPeeked = peekFor === row.id;
    const pendingConfirm = isPeeked ? confirmForRow(row) : undefined;
    return (
      <Box key={row.id} flexDirection="column">
        <Box flexDirection="row" paddingLeft={1}>
          <Text color={color}>
            {shownGlyph}
            {row.pinned ? ' ⌖' : '  '}
          </Text>
          <Text
            bold={isSelected}
            color={isSelected ? 'text' : 'subtle'}
            wrap="truncate-end"
          >
            {` ${truncate(row.title, titleWidth)}`}
          </Text>
          <Box flexGrow={1} />
          <Text dimColor wrap="truncate-end">
            {padLine(truncate(row.activityLine || row.state, activityWidth), activityWidth)}
          </Text>
        </Box>
        {isPeeked && (
          <Box flexDirection="column" paddingLeft={4} marginTop={0}>
            <Box flexDirection="row">
              <Text dimColor>
                {row.model ?? 'default model'}
                {row.permissionMode ? ` · ${row.permissionMode}` : ''}
                {` · ${STATE_LABEL[row.state] ?? row.state}`}
              </Text>
            </Box>
            {row.state === 'needs-input' &&
              (pendingConfirm ? (
                // The REAL permission dialog, inline — per-tool option
                // buttons with their 1-9 shortcuts work exactly as in the
                // main session. REPL's own copy doesn't mount while the
                // dashboard screen owns the render.
                <PermissionRequest
                  key={pendingConfirm.toolUseID}
                  toolUseConfirm={pendingConfirm}
                  toolUseContext={pendingConfirm.toolUseContext}
                  onDone={() => onPermissionDone?.(pendingConfirm.toolUseID)}
                  onReject={() => {}}
                  verbose={true}
                  workerBadge={pendingConfirm.workerBadge}
                />
              ) : (
                <Text color="warning">awaiting input — answer the prompt in the main session</Text>
              ))}
            {peek?.lastResponseText ? (
              peek.lastResponseText
                .split('\n')
                .filter(l => l.trim())
                .slice(0, 3)
                .map((line, i) => (
                  <Text key={i} dimColor wrap="truncate-end">
                    {truncate(line, columns - 8)}
                  </Text>
                ))
            ) : (
              <Text dimColor>{peek?.lastResponseType && peek.lastResponseType !== 'none' ? `last: ${peek.lastResponseType}` : 'no response yet'}</Text>
            )}
            <Box flexDirection="row" opaque>
              <Text color="gizzi">{'❯ '}</Text>
              <Text color={focus === 'reply' ? 'text' : 'inactive'}>
                {padLine(`${replyDraft}${focus === 'reply' ? '▌' : ''}`, columns - 6)}
              </Text>
            </Box>
          </Box>
        )}
      </Box>
    );
  };

  const renderCheatsheet = () => (
    <Box flexDirection="column" paddingLeft={1} marginTop={1} opaque>
      <Text bold color="gizzi">Dashboard keys</Text>
      <Text dimColor>{'  ↑/↓ select · Shift+↑/↓ reorder · Enter peek/reply'}</Text>
      <Text dimColor>{'  Tab dispatch · x stop (again: remove) · p pin · r rename'}</Text>
      <Text dimColor>{'  v details · / search (a: act, s: state, # id) · Ctrl+G group'}</Text>
      <Text dimColor>{'  Ctrl+S peek · Space collapse section · f factory floor · ? cheatsheet · Esc exit'}</Text>
    </Box>
  );

  const renderDetails = () => {
    const row = allRows.find(r => r.id === detailsFor);
    const transcript = detailsTranscript;
    // Screen header (1) + this title row (1) + footer (dispatch row + hint +
    // margin ≈ 3) + 1 spare row of slack.
    const detailsHeight = Math.max(5, termRows - 6);
    return (
      <Box flexDirection="column" paddingLeft={1} marginTop={1} opaque>
        <Box flexDirection="row" justifyContent="space-between">
          <Text bold color="gizzi" wrap="truncate-end">
            {truncate(row?.title ?? detailsFor ?? '', titleWidth)}
          </Text>
          <Text dimColor>{'↑/↓ scroll · Ctrl+U/D page · g/G top/bottom · Esc back'}</Text>
        </Box>
        {transcript.length === 0 ? (
          <Text dimColor>No transcript for this session.</Text>
        ) : (
          <ScrollBox
            ref={detailsScrollRef}
            height={detailsHeight}
            flexDirection="column"
            stickyScroll={true}
          >
            {/* The REAL Messages renderer (same component the REPL main
                screen and transcript view use) — full markdown, thinking
                blocks, tool chrome, grouping/collapse — not a text excerpt.
                screen='transcript' = unfiltered; the 200-message safety cap
                inside Messages bounds memory. stickyScroll keeps live
                sessions pinned to the newest message until scrolled. */}
            <Messages
              messages={transcript}
              tools={tools ?? []}
              commands={commands ?? []}
              verbose={true}
              toolJSX={null}
              toolUseConfirmQueue={TRANSCRIPT_EMPTY_ARR}
              inProgressToolUseIDs={TRANSCRIPT_NO_TOOL_USES}
              isMessageSelectorVisible={false}
              conversationId={detailsFor ?? 'dashboard-details'}
              screen="transcript"
              streamingToolUses={TRANSCRIPT_EMPTY_ARR}
              showAllInTranscript={true}
              isLoading={false}
              hideLogo={true}
              hidePastThinking={true}
            />
          </ScrollBox>
        )}
      </Box>
    );
  };

  const renderList = () => {
    // Window the items around the selection to fit the terminal.
    const maxItems = Math.max(5, termRows - 14);
    const flatIndexes = items.map((_, i) => i);
    const selFlatIdx = selItemIndex;
    let start = 0;
    if (selFlatIdx >= maxItems) start = selFlatIdx - maxItems + 1;
    const windowed = flatIndexes.slice(start, start + maxItems);
    return (
      <Box flexDirection="column" flexGrow={1} marginTop={1} opaque>
        {windowed.map(i => {
          const item = items[i];
          if (item.type === 'header') {
            return (
              <Box key={`h:${item.key}`} flexDirection="row" marginTop={0}>
                <Text dimColor>
                  {item.collapsed ? '▸' : '▾'} {item.label}
                </Text>
              </Box>
            );
          }
          if (item.type === 'more') {
            const isSelected = i === selItemIndex;
            return (
              <Box key="more" flexDirection="row" paddingLeft={1}>
                <Text dimColor>{'  '}</Text>
                <Text bold={isSelected} color={isSelected ? 'text' : 'inactive'}>
                  {`▸ ${item.count} more (Enter to show all)`}
                </Text>
              </Box>
            );
          }
          return renderRow(item, i);
        })}
      </Box>
    );
  };

  // ---------------------------------------------------------------------
  // Render
  // ---------------------------------------------------------------------

  return (
    <Box
      flexDirection="column"
      width={columns}
      height={termRows}
      paddingX={1}
    >
      <Box flexDirection="row" justifyContent="space-between">
        <Text bold color="gizzi">
          Gizzi Code · Dashboard
        </Text>
        <Text dimColor>
          {workingCount} agent{workingCount === 1 ? '' : 's'}
          {awaitingCount > 0 ? ` · ${awaitingCount} awaiting` : ''}
          {groupBy !== 'none' ? ` · grouped: ${groupBy}` : ''}
          {searchOpen ? ' · search' : ''}
        </Text>
      </Box>
      {searchOpen && (
        <Box flexDirection="row" marginTop={1} opaque>
          <Text color="gizzi">{'/ '}</Text>
          <Text color="text">
            {padLine(`${searchQuery}▌`, columns - 4)}
          </Text>
        </Box>
      )}
      {showCheatsheet ? renderCheatsheet() : detailsFor ? renderDetails() : renderList()}
      <Box flexDirection="column" marginTop={1}>
        {focus === 'rename' ? (
          <Box flexDirection="row" opaque>
            <Text color="gizzi">{'✎ '}</Text>
            <Text color="text">
              {padLine(`${renameDraft}▌`, columns - 4)}
            </Text>
          </Box>
        ) : (
          <Box flexDirection="row" opaque>
            <Text color="gizzi">{'❯ '}</Text>
            <Text color={focus === 'dispatch' ? 'text' : 'inactive'}>
              {padLine(`${draft}${focus === 'dispatch' ? '▌' : ''}`, columns - 4)}
            </Text>
          </Box>
        )}
        <Box flexDirection="row" justifyContent="space-between">
          <Text dimColor>
            {focus === 'dispatch'
              ? 'Enter dispatch · Ctrl+S dispatch → list · Esc back'
              : focus === 'reply'
                ? 'Enter reply · Ctrl+S send → list · Esc back'
                : focus === 'rename'
                  ? 'Enter rename · Esc cancel'
                  : focus === 'search'
                    ? 'type to filter · Esc clear'
                    : '? keys · Tab dispatch · Enter peek · / search · v details · Esc exit'}
          </Text>
          <Text dimColor>/dashboard</Text>
        </Box>
      </Box>
    </Box>
  );
}

export default DashboardScreen;
