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
import { buildModelPickerRows, getModelListSections, getModelOptions, refreshModelListSections, type ModelListSection, type ModelPickerRow } from '../utils/model/modelOptions';
import { formatContext, quotaMarker, toggleFavorite, visibleWindow, type PickerQuotaResult } from '../utils/model/modelPickerModel';
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
type OptionRow = Extract<ModelPickerRow, {
  kind: "option";
}>;
/** Rows (headers + options) rendered at once; the window follows focus. */
const ROW_WINDOW = 12;
/**
 * The command that signs a CLI provider in (`<bin> <catalog login args>`),
 * or undefined when the catalog has no login command for it. Same
 * require-in-render pattern as the quota lookup below.
 */
function loginCommandFor(providerId: string | undefined): string | undefined {
  if (!providerId) return undefined;
  try {
    const {
      findSubprocessSpec,
      CLI_CATALOG
    } = require('../../../../runtime/providers/discovery/subprocess.js') as typeof import('../../../../runtime/providers/discovery/subprocess.js');
    const spec = findSubprocessSpec(providerId);
    const tool = CLI_CATALOG.find(t => t.id === providerId) as {
      login?: unknown;
    } | undefined;
    const login = Array.isArray(tool?.login) ? tool!.login.filter((a): a is string => typeof a === 'string') : [];
    if (!spec || login.length === 0) return undefined;
    return [spec.bin, ...login].join(' ');
  } catch {
    return undefined;
  }
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
  // The shared model list (the rows `gizzi models` prints). Shown from the
  // last build at once, then rebuilt on open so new sign-ins and models show.
  const [sections, setSections] = useState<ModelListSection[]>(() => getModelListSections());
  const [loading, setLoading] = useState(true);
  useEffect(() => {
    let cancelled = false;
    void refreshModelListSections().then(next => {
      if (cancelled) return;
      setSections(next);
      setLoading(false);
    });
    return () => {
      cancelled = true;
    };
  }, []);
  const modelOptions = useMemo(() => getModelOptions(isFastMode ?? false), [isFastMode, sections]);
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
  // Options the shared list does not carry (env custom model, the running
  // model when no provider lists it) go under "Other".
  const extras = useMemo(() => optionsWithInitial.flatMap(opt => opt.value === null ? [] : [{
    value: opt.value,
    label: opt.label,
    description: opt.description
  }]), [optionsWithInitial]);
  const {
    rows,
    matched,
    total
  } = useMemo(() => buildModelPickerRows(sections, extras, {
    favorites,
    query: filter,
    // With no pick, the session runs the TUI's resolved default.
    current: initial ?? getDefaultMainLoopModel()
  }), [sections, extras, favorites, filter, initial]);
  const values = useMemo(() => rows.flatMap(r => r.kind === "option" ? [r.value] : []), [rows]);
  // With no model chosen, focus starts on the running default.
  const currentValue = rows.find((r): r is OptionRow => r.kind === "option" && r.isCurrent)?.value;
  // Focus can point at a row the filter hid — fall back to the current row, then the first match.
  const effectiveFocused = values.includes(focusedValue) ? focusedValue : currentValue !== undefined && values.includes(currentValue) ? currentValue : values[0];
  const focusedRow = rows.find((r): r is OptionRow => r.kind === "option" && r.value === effectiveFocused);
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
      for (const section of sections) {
        for (const provider of section.providers) {
          if (supported.has(provider.id)) ids.add(provider.id);
        }
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
  }, [sections]);
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
      {total === 0 && <Text dimColor={true}>{loading ? 'Loading models…' : 'No models yet. Run gizzi connect to add an account.'}</Text>}
      {total > 0 && matched === 0 && <Text dimColor={true}>No models match {JSON.stringify(filter)} — Backspace to edit, Esc to clear.</Text>}
      {windowRows.map(row => {
      if (row.kind === "section") {
        return <Box key={row.key}><Text bold={true}>{row.title}</Text></Box>;
      }
      if (row.kind === "provider") {
        const headerQuota = row.providerId ? quotaMarker(quotas[row.providerId], quotaFetchers.has(row.providerId)) : null;
        return <Box key={row.key} paddingLeft={1}><Text dimColor={true} bold={true}>{row.title}{headerQuota ? ` · ${headerQuota}` : ''}</Text></Box>;
      }
      const isFocused = row.value === effectiveFocused;
      const dim = row.signedIn === false && !isFocused;
      const tags = [row.isDefault ? 'default' : '', row.isCurrent ? 'current' : ''].filter(Boolean).join(' · ');
      const contextLabel = row.context ? formatContext(row.context) : null;
      const thinking = thinkingCell(row.value, isFocused ? displayEffort : undefined);
      return <Box key={row.key} flexDirection="row">
              <Text color={isFocused ? "suggestion" : undefined} dimColor={dim}>{isFocused ? figures.pointer : ' '}{' '}{row.favorite ? '★ ' : ''}{row.label}</Text>
              {tags !== '' && <Text dimColor={true}>{' '}{tags}</Text>}
              <Box flexGrow={1} />
              {contextLabel && <Text dimColor={true}>{contextLabel}</Text>}
              <Box width={THINKING_COLUMN} flexShrink={0} justifyContent="flex-end">
                <Text color={isFocused && thinking !== '—' ? "suggestion" : undefined} dimColor={!isFocused}>{isFocused && thinking !== '—' ? `‹ ${thinking} ›` : thinking}</Text>
              </Box>
            </Box>;
    })}
      {showCount && matched > 0 && <Text dimColor={true}>{matched} of {total} models{rows.length > ROW_WINDOW ? ' · ↑/↓ to scroll' : ''}</Text>}
    </Box>;
  // provider/model · context window · sign-in hint · quota, for the focused row.
  const focusedLogin = focusedRow?.signedIn === false ? loginCommandFor(focusedRow.providerId) : undefined;
  const focusedDetailParts = focusedRow ? [focusedRow.value, focusedRow.context ? `${formatContext(focusedRow.context)} context` : null, focusedRow.signedIn === false ? focusedLogin ? `not signed in — run ${focusedLogin}` : 'not signed in' : null, focusedRow.description && focusedRow.sectionTitle === 'Other' ? focusedRow.description : null, focusedQuota].filter((part): part is string => Boolean(part)) : [];
  const focusedDetail = focusedDetailParts.length > 0 ? <Box marginBottom={1}><Text dimColor={true}>{focusedDetailParts.join(' · ')}</Text></Box> : null;
  const effortRow = focusedThinking ? <Box marginBottom={1} flexDirection="column"><Text dimColor={true}><EffortLevelIndicator effort={displayEffort as EffortLevel} />{" "}{focusedThinking.term}:{" "}{focusedThinking.levels.map((l, i) => <Text key={l.id} bold={l.id === displayEffort} dimColor={l.id !== displayEffort}>{i > 0 ? ' · ' : ''}{l.label}{l.id === focusedThinking.default ? '*' : ''}</Text>)}{" "}<Text color="subtle">← → change · * default</Text></Text></Box> : <Box marginBottom={1} flexDirection="column">{focusedSupportsEffort ? <Text dimColor={true}><EffortLevelIndicator effort={displayEffort as EffortLevel} />{" "}{capitalize(displayEffort)} effort{displayEffort === focusedDefaultEffort ? " (default)" : ""}{" "}<Text color="subtle">← → to adjust</Text></Text> : <Text color="subtle"><EffortLevelIndicator effort={undefined} /> Effort not supported{focusedModelName ? ` for ${focusedModelName}` : ""}</Text>}</Box>;
  const header = <Box marginBottom={1} flexDirection="column"><Text color="remember" bold={true}>Select model</Text><Text dimColor={true}>{headerText ?? "Switch models. Applies to this session and future Gizzi Code sessions. For other/previous model names, specify with --model."}</Text>{sessionModel && <Text dimColor={true}>Currently using {modelDisplayString(sessionModel)} for this session (set by plan mode). Selecting a model will undo this.</Text>}</Box>;
  const footerHint = <Box marginBottom={1}><Text dimColor={true}>↑/↓ navigate · Enter select · Tab ★ favorite · type to filter · Esc {filter ? 'clear filter' : 'cancel'} · gizzi connect to add an account</Text></Box>;
  const fastModeNotice = isFastModeEnabled() ? showFastModeNotice ? <Box marginBottom={1}><Text dimColor={true}>Fast mode is <Text bold={true}>ON</Text> and available with{" "}{FAST_MODE_MODEL_DISPLAY} only (/fast). Switching to other models turn off fast mode.</Text></Box> : isFastModeAvailable() && !isFastModeCooldown() ? <Box marginBottom={1}><Text dimColor={true}>Use <Text bold={true}>/fast</Text> to turn on Fast mode ({FAST_MODE_MODEL_DISPLAY} only).</Text></Box> : null : null;
  const standaloneHints = isStandaloneCommand && <Text dimColor={true} italic={true}>{exitState.pending ? <>Press {exitState.keyName} again to exit</> : <Byline><KeyboardShortcutHint shortcut="Enter" action="confirm" /><ConfigurableShortcutHint action="select:cancel" context="Select" fallback="Esc" description="exit" /></Byline>}</Text>;
  const content = <Box flexDirection="column">{header}{listContent}{focusedDetail}{effortRow}{footerHint}{fastModeNotice}{standaloneHints}</Box>;
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
