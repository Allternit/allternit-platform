import capitalize from 'lodash-es/capitalize';
import figures from 'figures';
import * as React from 'react';
import { useEffect, useMemo, useState } from 'react';
import { useRegisterOverlay } from '../context/overlayContext.js';
import { useExitOnCtrlCDWithKeybindings } from './../hooks/useExitOnCtrlCDWithKeybindings.ts';
import { type AnalyticsMetadata_I_VERIFIED_THIS_IS_NOT_CODE_OR_FILEPATHS, logEvent } from './../services/analytics/index.ts';
import { FAST_MODE_MODEL_DISPLAY, isFastModeAvailable, isFastModeCooldown, isFastModeEnabled } from './../utils/fastMode.ts';
import { Box, Text, useInput } from '../ink';
import { useKeybindings } from '../keybindings/useKeybinding';
import { useAppState, useSetAppState } from '../state/AppState';
import { convertEffortValueToLevel, type EffortLevel, getDefaultEffortForModel, modelSupportsEffort, modelSupportsMaxEffort, resolvePickerEffortPersistence, toPersistableEffort } from '../utils/effort';
import { getDefaultMainLoopModel, type ModelSetting, modelDisplayString, parseUserSpecifiedModel } from '../utils/model/model';
import { getConfiguredProviderMeta, getModelOptions } from '../utils/model/modelOptions';
import { buildPickerRows, quotaMarker, selectableValues, toggleFavorite, visibleWindow, type PickerProviderMeta, type PickerQuotaResult, type PickerRow } from '../utils/model/modelPickerModel';
import { getSettingsForSource, updateSettingsForSource } from '../utils/settings/settings';
import { ConfigurableShortcutHint } from './ConfigurableShortcutHint';
import { Byline } from './design-system/Byline';
import { KeyboardShortcutHint } from './design-system/KeyboardShortcutHint';
import { Pane } from './design-system/Pane';
import { effortLevelToSymbol } from './EffortIndicator';
import { ModelThinking } from '@/runtime/providers/discovery/model-thinking';
export type Props = {
  initial: string | null;
  sessionModel?: ModelSetting;
  onSelect: (model: string | null, effort: EffortLevel | undefined) => void;
  onCancel?: () => void;
  isStandaloneCommand?: boolean;
  showFastModeNotice?: boolean;
  /** Overrides the dim header line below "Select model". */
  headerText?: string;
  /**
   * When true, skip writing effortLevel to userSettings on selection.
   * Used by the assistant installer wizard where the model choice is
   * project-scoped (written to the assistant's .claude/settings.json via
   * install.ts) and should not leak to the user's global ~/.claude/settings.
   */
  skipSettingsWrite?: boolean;
};
const NO_PREFERENCE = '__NO_PREFERENCE__';
/** Rows (headers + options) rendered at once; the window follows focus. */
const ROW_WINDOW = 12;
/**
 * Provider metadata for every discovered `${providerId}/${modelId}` option
 * value, used to group the picker into sections and show per-row context
 * windows. Same require-in-render pattern as modelOptions.ts — Discovery is
 * a runtime module that must not break the picker if unavailable.
 */
function getDiscoveryMeta(): Map<string, PickerProviderMeta> {
  const map = new Map<string, PickerProviderMeta>();
  try {
    const { Discovery } = require('../../../../runtime/providers/discovery/index.js') as typeof import('../../../../runtime/providers/discovery/index.js');
    const { ProviderOffer } = require('../../../../runtime/providers/offer.js') as typeof import('../../../../runtime/providers/offer.js');
    Discovery.prefetch();
    for (const dp of Discovery.last()) {
      const kind = ProviderOffer.kind(dp);
      for (const m of dp.models) {
        map.set(`${dp.id}/${m.id}`, {
          providerId: dp.id,
          providerName: dp.name,
          source: dp.source,
          kind,
          context: m.context,
          output: m.output,
        });
      }
    }
  } catch {}
  // API keys and local servers from gizzi.json (discovery wins for the same model).
  for (const [value, meta] of getConfiguredProviderMeta()) {
    if (!map.has(value)) map.set(value, meta);
  }
  return map;
}
export function ModelPicker({
    initial,
    sessionModel,
    onSelect,
    onCancel,
    isStandaloneCommand,
    showFastModeNotice,
    headerText,
    skipSettingsWrite
}: Props) {
  const setAppState = useSetAppState();
  const exitState = useExitOnCtrlCDWithKeybindings();
  const initialValue = initial === null ? NO_PREFERENCE : initial;
  const [focusedValue, setFocusedValue] = useState(initialValue);
  const [filter, setFilter] = useState('');
  const [favorites, setFavorites] = useState<string[]>(() => getSettingsForSource("userSettings")?.modelFavorites ?? []);
  const [quotas, setQuotas] = useState<Record<string, PickerQuotaResult>>({});
  const isFastMode = useAppState(s => isFastModeEnabled() ? s.fastMode : false);
  const [hasToggledEffort, setHasToggledEffort] = useState(false);
  const effortValue = useAppState(s => s.effortValue);
  // A thinking level id: Claude's low…max, or any level in model-thinking.json (xhigh, minimal, ultra…).
  const [effort, setEffort] = useState<string | undefined>(effortValue === undefined ? undefined : typeof effortValue === 'string' ? effortValue : convertEffortValueToLevel(effortValue));
  const metaMap = useMemo(() => getDiscoveryMeta(), []);
  const modelOptions = useMemo(() => getModelOptions(isFastMode ?? false), [isFastMode]);
  const optionsWithInitial = useMemo(() => {
    if (initial !== null && !modelOptions.some(opt => opt.value === initial)) {
      return [...modelOptions, {
        value: initial,
        label: modelDisplayString(initial),
        description: "Current model"
      }];
    }
    return modelOptions;
  }, [modelOptions, initial]);
  const selectOptions = useMemo(() => optionsWithInitial.map(opt => ({
    ...opt,
    value: opt.value === null ? NO_PREFERENCE : opt.value
  })), [optionsWithInitial]);
  const {
    rows,
    matched,
    total
  } = useMemo(() => buildPickerRows(selectOptions, {
    metaFor: value => metaMap.get(value),
    favorites,
    query: filter
  }), [selectOptions, metaMap, favorites, filter]);
  const values = useMemo(() => selectableValues(rows), [rows]);
  // Focus can point at a row the filter hid — fall back to the first match.
  const effectiveFocused = values.includes(focusedValue) ? focusedValue : values[0];
  const focusedRow = rows.find((r): r is Extract<PickerRow, {
    kind: "option";
  }> => r.kind === "option" && r.value === effectiveFocused);
  const focusIndex = rows.findIndex(r => r.kind === "option" && r.value === effectiveFocused);
  const windowRows = visibleWindow(rows, focusIndex === -1 ? 0 : focusIndex, ROW_WINDOW);

  // Providers in the list that have a real quota fetcher; everything else
  // renders the explicit "quota n/a" marker instead of an empty slot.
  const quotaFetchers = useMemo(() => {
    const set = new Set<string>();
    try {
      const {
        ProviderQuotas
      } = require('../../../../runtime/providers/quota/index.js') as typeof import('../../../../runtime/providers/quota/index.js');
      for (const id of ProviderQuotas.supported()) set.add(id);
    } catch {}
    return set;
  }, []);

  // Lazily pull plan quotas for providers in the list that report them.
  // Never blocks rendering; providers without a quota source show the
  // "quota n/a" marker, never a fabricated number.
  useEffect(() => {
    let cancelled = false;
    try {
      const {
        ProviderQuotas
      } = require('../../../../runtime/providers/quota/index.js') as typeof import('../../../../runtime/providers/quota/index.js');
      const supported = new Set(ProviderQuotas.supported());
      const ids = new Set<string>();
      for (const meta of metaMap.values()) {
        if (supported.has(meta.providerId)) ids.add(meta.providerId);
      }
      for (const id of ids) {
        void ProviderQuotas.get(id).then(result => {
          if (!cancelled) setQuotas(prev => ({
            ...prev,
            [id]: result
          }));
        }).catch(() => {});
      }
    } catch {}
    return () => {
      cancelled = true;
    };
  }, [metaMap]);
  const focusedModelName = focusedRow?.label;
  const focusedModel = resolveOptionModel(effectiveFocused);
  const focusedSupportsEffort = focusedModel ? modelSupportsEffort(focusedModel) : false;
  const focusedSupportsMax = focusedModel ? modelSupportsMaxEffort(focusedModel) : false;
  // The shared per-model thinking table (the same one Desktop, web and iOS pick from).
  const focusedThinking = focusedModel ? ModelThinking.describe(focusedModel) : undefined;
  const focusedDefaultEffort = getDefaultEffortLevelForOption(effectiveFocused);
  const displayEffort = focusedThinking ? ModelThinking.check(focusedModel!, effort) ?? focusedDefaultEffort : effort === "max" && !focusedSupportsMax ? "high" : effort;
  const handleFocus = (value: string) => {
    setFocusedValue(value);
    if (!hasToggledEffort && effortValue === undefined) {
      setEffort(getDefaultEffortLevelForOption(value));
    }
  };
  const moveFocus = (direction: 1 | -1) => {
    if (values.length === 0) return;
    const idx = effectiveFocused !== undefined ? values.indexOf(effectiveFocused) : -1;
    const next = values[(idx + direction + values.length) % values.length]!;
    handleFocus(next);
  };
  const handleCycleEffort = (direction: 'left' | 'right') => {
    if (!focusedSupportsEffort) {
      return;
    }
    if (focusedThinking) {
      const ids = focusedThinking.levels.map(l => l.id);
      setEffort(prev => {
        const at = ids.indexOf(ModelThinking.check(focusedModel!, prev) ?? focusedDefaultEffort);
        const from = at === -1 ? ids.indexOf(focusedThinking.default) : at;
        return ids[direction === 'right' ? Math.min(ids.length - 1, from + 1) : Math.max(0, from - 1)];
      });
    } else {
      setEffort(prev => cycleEffortLevel((prev ?? focusedDefaultEffort) as EffortLevel, direction, focusedSupportsMax));
    }
    setHasToggledEffort(true);
  };
  useKeybindings({
    "modelPicker:decreaseEffort": () => handleCycleEffort("left"),
    "modelPicker:increaseEffort": () => handleCycleEffort("right")
  }, {
    context: "ModelPicker"
  });
  const handleSelect = (value_0: string) => {
    logEvent("tengu_model_command_menu_effort", {
      effort: effort as AnalyticsMetadata_I_VERIFIED_THIS_IS_NOT_CODE_OR_FILEPATHS
    });
    if (!skipSettingsWrite) {
      const effortLevel = resolvePickerEffortPersistence(effort as EffortLevel | undefined, getDefaultEffortLevelForOption(value_0) as EffortLevel, getSettingsForSource("userSettings")?.effortLevel, hasToggledEffort);
      const persistable = toPersistableEffort(effortLevel);
      if (persistable !== undefined) {
        updateSettingsForSource("userSettings", {
          effortLevel: persistable
        });
      }
      setAppState(prev_0 => ({
        ...prev_0,
        effortValue: effortLevel
      }));
    }
    const selectedModel = resolveOptionModel(value_0);
    const selectedEffort = (hasToggledEffort && selectedModel && modelSupportsEffort(selectedModel) ? effort : undefined) as EffortLevel | undefined;
    if (value_0 === NO_PREFERENCE) {
      onSelect(null, selectedEffort);
      return;
    }
    onSelect(value_0, selectedEffort);
  };
  const handleToggleFavorite = () => {
    if (effectiveFocused === undefined) return;
    const next = toggleFavorite(favorites, effectiveFocused);
    setFavorites(next);
    updateSettingsForSource("userSettings", {
      modelFavorites: next
    });
  };
  const cancel = onCancel ?? (() => {});
  // Register as an overlay so Escape reaches this picker instead of the
  // global cancel handler (same pattern as CustomSelect).
  useRegisterOverlay('model-picker', true);
  useInput((input, key) => {
    if (key.upArrow) {
      moveFocus(-1);
      return;
    }
    if (key.downArrow) {
      moveFocus(1);
      return;
    }
    if (key.return) {
      if (effectiveFocused !== undefined) handleSelect(effectiveFocused);
      return;
    }
    if (key.escape) {
      if (filter) {
        setFilter('');
      } else {
        cancel();
      }
      return;
    }
    if (key.tab) {
      handleToggleFavorite();
      return;
    }
    // ← → belong to the effort keybindings above.
    if (key.leftArrow || key.rightArrow) return;
    if (key.backspace || key.delete) {
      setFilter(prev => prev.slice(0, -1));
      return;
    }
    if (key.ctrl || key.meta) return;
    if (input) {
      setFilter(prev => prev + input);
    }
  });
  const focusedQuota = focusedRow?.providerId ? quotaMarker(quotas[focusedRow.providerId], quotaFetchers.has(focusedRow.providerId)) : null;
  const showCount = filter !== '' || total > ROW_WINDOW;
  const listContent = <Box flexDirection="column" marginBottom={1}>
      {filter !== '' && <Text>Filter: <Text bold={true}>{filter}</Text>{matched === 0 ? '' : ` (${matched} of ${total})`}</Text>}
      {matched === 0 && <Text dimColor={true}>No models match {JSON.stringify(filter)} — Backspace to edit, Esc to clear.</Text>}
      {windowRows.map(row => {
      if (row.kind === "header") {
        const headerQuota = row.providerId ? quotaMarker(quotas[row.providerId], quotaFetchers.has(row.providerId)) : null;
        return <Box key={row.key}><Text dimColor={true} bold={true}>{row.title}{headerQuota ? ` · ${headerQuota}` : ''}</Text></Box>;
      }
      const isFocused = row.value === effectiveFocused;
      const isCurrent = row.value === initialValue;
      const meta = [row.contextLabel, ...row.badges].filter(Boolean).join(' · ');
      const thinking = thinkingCell(row.value, isFocused ? displayEffort : undefined);
      return <Box key={row.key} flexDirection="row">
              <Text color={isFocused ? "suggestion" : undefined}>{isFocused ? figures.pointer : ' '}{' '}{row.favorite ? '★ ' : ''}{row.label}{isCurrent ? ` ${figures.tick}` : ''}</Text>
              <Box flexGrow={1} />
              {meta !== '' && <Text dimColor={true}>{meta}</Text>}
              <Box width={THINKING_COLUMN} flexShrink={0} justifyContent="flex-end">
                <Text color={isFocused && thinking !== '—' ? "suggestion" : undefined} dimColor={!isFocused}>{isFocused && thinking !== '—' ? `‹ ${thinking} ›` : thinking}</Text>
              </Box>
            </Box>;
    })}
      {showCount && matched > 0 && <Text dimColor={true}>{matched} of {total} models{rows.length > ROW_WINDOW ? ' · ↑/↓ to scroll' : ''}</Text>}
    </Box>;
  const focusedDetail = focusedRow && (focusedRow.description || focusedQuota) ? <Box marginBottom={1}><Text dimColor={true}>{focusedRow.description}{focusedRow.description && focusedQuota ? ' · ' : ''}{focusedQuota ?? ''}</Text></Box> : null;
  const effortRow = focusedThinking ? <Box marginBottom={1} flexDirection="column"><Text dimColor={true}><EffortLevelIndicator effort={displayEffort as EffortLevel} />{" "}{focusedThinking.term}:{" "}{focusedThinking.levels.map((l, i) => <Text key={l.id} bold={l.id === displayEffort} dimColor={l.id !== displayEffort}>{i > 0 ? ' · ' : ''}{l.label}{l.id === focusedThinking.default ? '*' : ''}</Text>)}{" "}<Text color="subtle">← → change · * default</Text></Text></Box> : <Box marginBottom={1} flexDirection="column">{focusedSupportsEffort ? <Text dimColor={true}><EffortLevelIndicator effort={displayEffort as EffortLevel} />{" "}{capitalize(displayEffort)} effort{displayEffort === focusedDefaultEffort ? " (default)" : ""}{" "}<Text color="subtle">← → to adjust</Text></Text> : <Text color="subtle"><EffortLevelIndicator effort={undefined} /> Effort not supported{focusedModelName ? ` for ${focusedModelName}` : ""}</Text>}</Box>;
  const header = <Box marginBottom={1} flexDirection="column"><Text color="remember" bold={true}>Select model</Text><Text dimColor={true}>{headerText ?? "Switch models. Applies to this session and future Gizzi Code sessions. For other/previous model names, specify with --model."}</Text>{sessionModel && <Text dimColor={true}>Currently using {modelDisplayString(sessionModel)} for this session (set by plan mode). Selecting a model will undo this.</Text>}<Text dimColor={true}>↑/↓ navigate · Enter select · Tab ★ favorite · type to filter · Esc {filter ? 'clear filter' : 'cancel'}</Text></Box>;
  const fastModeNotice = isFastModeEnabled() ? showFastModeNotice ? <Box marginBottom={1}><Text dimColor={true}>Fast mode is <Text bold={true}>ON</Text> and available with{" "}{FAST_MODE_MODEL_DISPLAY} only (/fast). Switching to other models turn off fast mode.</Text></Box> : isFastModeAvailable() && !isFastModeCooldown() ? <Box marginBottom={1}><Text dimColor={true}>Use <Text bold={true}>/fast</Text> to turn on Fast mode ({FAST_MODE_MODEL_DISPLAY} only).</Text></Box> : null : null;
  const standaloneHints = isStandaloneCommand && <Text dimColor={true} italic={true}>{exitState.pending ? <>Press {exitState.keyName} again to exit</> : <Byline><KeyboardShortcutHint shortcut="Enter" action="confirm" /><ConfigurableShortcutHint action="select:cancel" context="Select" fallback="Esc" description="exit" /></Byline>}</Text>;
  const content = <Box flexDirection="column">{header}{listContent}{focusedDetail}{effortRow}{fastModeNotice}{standaloneHints}</Box>;
  if (!isStandaloneCommand) {
    return content;
  }
  return <Pane color="permission">{content}</Pane>;
}
function resolveOptionModel(value?: string): string | undefined {
  if (!value) return undefined;
  return value === NO_PREFERENCE ? getDefaultMainLoopModel() : parseUserSpecifiedModel(value);
}
function EffortLevelIndicator(t0: {
  effort: EffortLevel | undefined;
}) {
  const {
    effort
  } = t0;
  return <Text color={effort ? "gizzi" : "subtle"}>{effortLevelToSymbol(effort ?? "low")}</Text>;
}
function cycleEffortLevel(current: EffortLevel, direction: 'left' | 'right', includeMax: boolean): EffortLevel {
  const levels: EffortLevel[] = includeMax ? ['low', 'medium', 'high', 'max'] : ['low', 'medium', 'high'];
  // If the current level isn't in the cycle (e.g. 'max' after switching to a
  // non-Opus model), clamp to 'high'.
  const idx = levels.indexOf(current);
  const currentIndex = idx !== -1 ? idx : levels.indexOf('high');
  if (direction === 'right') {
    return levels[(currentIndex + 1) % levels.length]!;
  } else {
    return levels[(currentIndex - 1 + levels.length) % levels.length]!;
  }
}
const THINKING_COLUMN = 16;

/**
 * The thinking column for one row: the focused row's chosen level, else the
 * model's default, in its vendor's words. "—" when the shared table lists
 * no levels for the model (it runs at its own default).
 */
function thinkingCell(value: string | undefined, chosen: string | undefined): string {
  const model = resolveOptionModel(value);
  const described = model ? ModelThinking.describe(model) : undefined;
  if (!described) return '—';
  const id = (model && ModelThinking.check(model, chosen)) || described.default;
  return described.levels.find(l => l.id === id)?.label ?? id;
}
function getDefaultEffortLevelForOption(value?: string): string {
  const resolved = resolveOptionModel(value) ?? getDefaultMainLoopModel();
  const table = ModelThinking.describe(resolved);
  if (table) return table.default;
  const defaultValue = getDefaultEffortForModel(resolved);
  return defaultValue !== undefined ? convertEffortValueToLevel(defaultValue) : 'high';
}
