/**
 * `/runtime` picker (Harness & Bots 2.0, plan D7 / step 9).
 *
 * Two layers in the terminal's shape: first Runtime (how it works: the
 * registry's `runtimes[]`, grouped by category, "More" ones behind a row),
 * then Brain (how it thinks: the shared model list, only brains that fit the
 * runtime, each with its access badge). Picking a runtime that can't run the
 * current brain switches the brain to the first one that fits and says so in
 * one line. A Connected bot shows both layers locked: "Run by <Vendor>".
 *
 * The compatibility rule, the auto-switch and the record write all come from
 * `gizzi agents bot edit --runtime/--brain` (cli/commands/bot.ts), so the TUI
 * and the CLI can't drift.
 */
import figures from 'figures';
import * as React from 'react';
import { useEffect, useMemo, useState } from 'react';
import { useRegisterOverlay } from '../context/overlayContext.js';
import { Box, Text, useInput } from '../ink';
import { getModelListSections, refreshModelListSections, type ModelListSection } from '../utils/model/modelOptions';
import { visibleWindow } from '../utils/model/modelPickerModel';
import { Pane } from './design-system/Pane';
import {
  fits,
  getRuntime,
  listRuntimes,
  type AccessKind,
  type RuntimeEntry,
} from '@/runtime/providers/discovery/runtime-registry';
import { CATEGORY_LABEL, CATEGORY_ORDER, runtimeInstalled } from '@/cli/commands/runtime/catalog';

export type RuntimePick = {
  runtime: RuntimeEntry;
  /** provider/model */
  brain: string;
  /** The one-line auto-switch note, when the brain changed to fit the runtime. */
  note?: string;
};

export type Props = {
  /** Runtime id the session or bot runs on now. */
  initialRuntime: string;
  /** Runtime to focus first (`/runtime <id>`); defaults to the current one. */
  focusRuntime?: string;
  /** provider/model the session or bot thinks with now, if known. */
  initialBrain: string | null;
  /** Who the sentence talks about: the bot's name, or "This session". */
  subject: string;
  /**
   * The auto-switch rule for a runtime pick (bot.ts applyRuntimeBrain):
   * returns the brain to focus and the note, or throws with a message.
   */
  planRuntime: (runtime: RuntimeEntry) => { brain: string | null; note?: string };
  onSelect: (pick: RuntimePick) => void;
  onCancel: () => void;
};

const ROW_WINDOW = 12;

/** Section kind of the shared model list → access badge. */
const ACCESS_OF_KIND: Record<string, AccessKind> = {
  cloud: 'credits',
  subscription: 'plan',
  api: 'key',
  local: 'local',
};
const ACCESS_LABEL: Record<AccessKind, string> = { plan: 'plan', key: 'key', local: 'local', credits: 'credits' };
const ACCESS_VIA: Record<AccessKind, string> = {
  plan: 'on your plan',
  key: 'with your API key',
  local: 'on this computer',
  credits: 'on Allternit credits',
};

type RuntimeRow =
  | { kind: 'header'; key: string; title: string }
  | { kind: 'runtime'; key: string; r: RuntimeEntry }
  | { kind: 'more'; key: string; count: number };

type BrainRow =
  | { kind: 'header'; key: string; title: string }
  | {
      kind: 'brain';
      key: string;
      ref: string;
      name: string;
      providerName: string;
      access?: AccessKind;
      signedIn?: boolean;
    };

/** "Any brain", or "Anthropic models" for a runtime that runs one vendor's models. */
function brainsShort(r: RuntimeEntry): string {
  if (r.brains === 'any') return 'any brain';
  return `${r.brains.map((b) => b.charAt(0).toUpperCase() + b.slice(1)).join(' or ')} models`;
}

function matches(query: string, hay: Array<string | undefined>): boolean {
  return !query || hay.filter(Boolean).join(' ').toLowerCase().includes(query.toLowerCase());
}

export function RuntimePicker({ initialRuntime, focusRuntime, initialBrain, subject, planRuntime, onSelect, onCancel }: Props) {
  const [layer, setLayer] = useState<'runtime' | 'brain'>('runtime');
  const [filter, setFilter] = useState('');
  const [showMore, setShowMore] = useState(() => Boolean(getRuntime(focusRuntime ?? initialRuntime)?.more));
  const [runtimeFocus, setRuntimeFocus] = useState<string>(getRuntime(focusRuntime ?? initialRuntime)?.id ?? 'allternit');
  const [runtime, setRuntime] = useState<RuntimeEntry | undefined>(getRuntime(initialRuntime));
  const [brainFocus, setBrainFocus] = useState<string | null>(initialBrain);
  // The brain the runtime pick settled on (current, or auto-switched).
  const [plannedBrain, setPlannedBrain] = useState<string | null>(initialBrain);
  const [note, setNote] = useState<string>();
  const [error, setError] = useState<string>();
  const [installed, setInstalled] = useState<Record<string, boolean | null>>({});
  const [sections, setSections] = useState<ModelListSection[]>(() => getModelListSections());
  const [loading, setLoading] = useState(true);

  // Same CLI lookup `gizzi runtime catalog` uses; never blocks rendering.
  useEffect(() => {
    let cancelled = false;
    const every = listRuntimes({ includeMore: true });
    void Promise.all(every.map(runtimeInstalled)).then((res) => {
      if (cancelled) return;
      setInstalled(Object.fromEntries(every.map((r, i) => [r.id, res[i] ?? null])));
    }).catch(() => {});
    return () => {
      cancelled = true;
    };
  }, []);
  // The shared model list (the rows `gizzi models` and /model show).
  useEffect(() => {
    let cancelled = false;
    void refreshModelListSections().then((next) => {
      if (cancelled) return;
      setSections(next);
      setLoading(false);
    });
    return () => {
      cancelled = true;
    };
  }, []);

  const runtimeRows = useMemo<RuntimeRow[]>(() => {
    const rows: RuntimeRow[] = [];
    const every = listRuntimes({ includeMore: true }).filter((r) => matches(filter, [r.id, r.name, r.vendor, ...(r.aliases ?? [])]));
    for (const cat of CATEGORY_ORDER) {
      const inCat = every.filter((r) => r.category === cat && !r.more);
      if (inCat.length === 0) continue;
      rows.push({ kind: 'header', key: `h:${cat}`, title: CATEGORY_LABEL[cat] });
      for (const r of inCat) rows.push({ kind: 'runtime', key: r.id, r });
    }
    const more = every.filter((r) => r.more);
    if (more.length > 0) {
      // Typing a filter opens "More" so a match is never hidden behind it.
      if (showMore || filter) {
        rows.push({ kind: 'header', key: 'h:more', title: 'More' });
        for (const r of more) rows.push({ kind: 'runtime', key: r.id, r });
      } else {
        rows.push({ kind: 'more', key: 'more', count: more.length });
      }
    }
    return rows;
  }, [filter, showMore]);

  const brainRows = useMemo<BrainRow[]>(() => {
    if (!runtime) return [];
    const rows: BrainRow[] = [];
    for (const section of sections) {
      const access = ACCESS_OF_KIND[section.kind];
      const inSection: BrainRow[] = [];
      for (const provider of section.providers) {
        if (!fits(runtime, provider.id)) continue;
        for (const m of provider.models) {
          if (m.hidden) continue;
          if (!matches(filter, [m.ref, m.name, provider.name])) continue;
          inSection.push({
            kind: 'brain',
            key: m.ref,
            ref: m.ref,
            name: m.name,
            providerName: provider.name,
            access,
            signedIn: provider.signedIn,
          });
        }
      }
      if (inSection.length === 0) continue;
      rows.push({ kind: 'header', key: `h:${section.kind}`, title: section.title }, ...inSection);
    }
    // The planned brain (current or auto-switched) always shows, even when the
    // shared list has not built it yet.
    if (plannedBrain && !filter && !rows.some((r) => r.kind === 'brain' && r.ref === plannedBrain)) {
      rows.push({ kind: 'header', key: 'h:other', title: 'Other' });
      rows.push({ kind: 'brain', key: plannedBrain, ref: plannedBrain, name: plannedBrain, providerName: plannedBrain.split('/')[0] ?? '' });
    }
    return rows;
  }, [runtime, sections, filter, plannedBrain]);

  const rows: Array<RuntimeRow | BrainRow> = layer === 'runtime' ? runtimeRows : brainRows;
  const selectable = rows.filter((r) => r.kind !== 'header');
  const focusKey = layer === 'runtime' ? runtimeFocus : brainFocus;
  const focused = selectable.find((r) => r.key === focusKey) ?? selectable[0];
  const focusIndex = focused ? rows.indexOf(focused) : 0;
  const windowRows = visibleWindow(rows, Math.max(0, focusIndex), ROW_WINDOW);

  const setFocus = (key: string) => {
    if (layer === 'runtime') setRuntimeFocus(key);
    else setBrainFocus(key);
  };
  const moveFocus = (direction: 1 | -1) => {
    if (selectable.length === 0) return;
    const idx = focused ? selectable.indexOf(focused) : -1;
    setFocus(selectable[(idx + direction + selectable.length) % selectable.length]!.key);
  };

  const pickRuntime = (r: RuntimeEntry) => {
    try {
      const plan = planRuntime(r);
      setRuntime(r);
      setBrainFocus(plan.brain);
      setPlannedBrain(plan.brain);
      setNote(plan.note);
      setError(undefined);
      setFilter('');
      setLayer('brain');
    } catch (e) {
      setError((e as Error).message);
    }
  };

  const confirm = () => {
    if (!focused) return;
    if (focused.kind === 'more') {
      setShowMore(true);
      return;
    }
    if (focused.kind === 'runtime') {
      pickRuntime(focused.r);
      return;
    }
    if (focused.kind === 'brain' && runtime) {
      onSelect({ runtime, brain: focused.ref, note: focused.ref === plannedBrain ? note : undefined });
    }
  };

  useRegisterOverlay('runtime-picker', true);
  useInput((input, key) => {
    if (key.upArrow) return moveFocus(-1);
    if (key.downArrow) return moveFocus(1);
    if (key.return) return confirm();
    if (key.escape) {
      if (filter) setFilter('');
      else if (layer === 'brain') {
        setLayer('runtime');
        setNote(undefined);
      } else onCancel();
      return;
    }
    if (key.leftArrow && layer === 'brain' && !filter) {
      setLayer('runtime');
      setNote(undefined);
      return;
    }
    if (key.backspace || key.delete) return setFilter((prev) => prev.slice(0, -1));
    if (key.ctrl || key.meta || key.tab || key.leftArrow || key.rightArrow) return;
    if (input) setFilter((prev) => prev + input);
  });

  // Closed layers show as one line (the chip); the open layer shows its choices.
  const runtimeName = runtime?.name ?? getRuntime(runtimeFocus)?.name ?? runtimeFocus;
  const focusedBrain = focused && focused.kind === 'brain' ? focused : undefined;
  const chip = (label: string, value: string, hint: string, open: boolean) => (
    <Box>
      <Text color={open ? 'suggestion' : undefined} bold={open}>{open ? figures.pointer : figures.tick} {label}</Text>
      <Text dimColor={true}> · {hint}{open ? '' : ' · '}</Text>
      {!open && <Text>{value}</Text>}
    </Box>
  );

  const list = (
    <Box flexDirection="column" marginLeft={2} marginBottom={1}>
      {filter !== '' && <Text>Filter: <Text bold={true}>{filter}</Text></Text>}
      {layer === 'brain' && selectable.length === 0 && (
        <Text dimColor={true}>{loading ? 'Loading brains…' : filter ? `No brains match ${JSON.stringify(filter)}.` : `No ${runtime ? brainsShort(runtime) : 'brains'} yet. Run gizzi connect to add an account.`}</Text>
      )}
      {layer === 'runtime' && selectable.length === 0 && <Text dimColor={true}>No runtimes match {JSON.stringify(filter)}.</Text>}
      {windowRows.map((row) => {
        if (row.kind === 'header') return <Text key={row.key} bold={true}>{row.title}</Text>;
        const isFocused = row === focused;
        const pointer = isFocused ? figures.pointer : ' ';
        if (row.kind === 'more') {
          return <Text key={row.key} color={isFocused ? 'suggestion' : undefined}>{pointer} More ({row.count}) <Text dimColor={true}>· Enter to show</Text></Text>;
        }
        if (row.kind === 'runtime') {
          const r = row.r;
          const inst = installed[r.id];
          const local = inst === true ? (r.adapter.kind === 'native' ? 'built in' : 'installed') : inst === false ? 'not installed' : '';
          const current = r.id === runtime?.id;
          return (
            <Box key={row.key}>
              <Text color={isFocused ? 'suggestion' : undefined} dimColor={inst === false && !isFocused}>{pointer} {r.name}</Text>
              <Text dimColor={true}> · {r.vendor} · {brainsShort(r)}{local ? ` · ${local}` : ''}{current ? ' · current' : ''}</Text>
              {r.status && <Text color={r.status.tone === 'warn' ? 'warning' : undefined} dimColor={r.status.tone !== 'warn'}> · {r.status.label}</Text>}
            </Box>
          );
        }
        return (
          <Box key={row.key}>
            <Text color={isFocused ? 'suggestion' : undefined} dimColor={row.signedIn === false && !isFocused}>{pointer} {row.name}</Text>
            <Text dimColor={true}> · {row.providerName}{row.ref === initialBrain ? ' · current' : ''}</Text>
            <Box flexGrow={1} />
            {row.access && <Text dimColor={!isFocused}>[{ACCESS_LABEL[row.access]}]</Text>}
          </Box>
        );
      })}
    </Box>
  );

  const brainLabel = focusedBrain?.name ?? brainFocus ?? initialBrain ?? 'its default brain';
  const via = focusedBrain?.access ? `, ${ACCESS_VIA[focusedBrain.access]}` : '';
  const sentence = `${subject} works with ${runtimeName} and thinks with ${brainLabel}${via}.`;

  return (
    <Pane color="permission">
      <Box flexDirection="column">
        <Box marginBottom={1} flexDirection="column">
          <Text color="remember" bold={true}>Runtime and brain</Text>
          <Text dimColor={true}>The runtime is how it works. The brain is how it thinks.</Text>
        </Box>
        {chip('Runtime', runtimeName, 'How it works', layer === 'runtime')}
        {layer === 'runtime' && list}
        {chip('Brain', layer === 'runtime' ? (initialBrain ?? 'default') : brainLabel, 'How it thinks', layer === 'brain')}
        {layer === 'brain' && list}
        {note && layer === 'brain' && <Box marginTop={1}><Text color="warning" wrap="wrap">{note}</Text></Box>}
        {error && <Box marginTop={1}><Text color="error" wrap="wrap">{error}</Text></Box>}
        <Box marginTop={1}><Text dimColor={true}>{sentence}</Text></Box>
        <Text dimColor={true}>
          ↑/↓ navigate · Enter {layer === 'runtime' ? 'choose runtime' : 'choose brain'} · type to filter · Esc {filter ? 'clear filter' : layer === 'brain' ? 'back to runtime' : 'cancel'}
        </Text>
      </Box>
    </Pane>
  );
}

/** A Connected bot: both layers locked to its vendor. */
export function RuntimeLocked({ botName, vendor, onDone }: { botName: string; vendor: string; onDone: () => void }) {
  useRegisterOverlay('runtime-picker', true);
  useInput((_input, key) => {
    if (key.escape || key.return) onDone();
  });
  return (
    <Pane color="permission">
      <Box flexDirection="column">
        <Box marginBottom={1}><Text color="remember" bold={true}>Runtime and brain</Text></Box>
        <Text>  Runtime <Text dimColor={true}>· How it works · </Text>Run by {vendor} <Text dimColor={true}>(locked)</Text></Text>
        <Text>  Brain <Text dimColor={true}>· How it thinks · </Text>Run by {vendor} <Text dimColor={true}>(locked)</Text></Text>
        <Box marginTop={1}><Text dimColor={true}>{botName} is run by {vendor}. Allternit shows its work and keeps the record.</Text></Box>
        <Text dimColor={true}>Enter or Esc to close</Text>
      </Box>
    </Pane>
  );
}
