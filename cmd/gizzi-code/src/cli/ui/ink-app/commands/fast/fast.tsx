import * as React from 'react';
import { useState } from 'react';
import type { CommandResultDisplay, LocalJSXCommandContext } from '../../commands';
import { Dialog } from '../../components/design-system/Dialog';
import { FastIcon, getFastIconString } from '../../components/FastIcon';
import { Box, Link, Text } from '../../ink';
import { useKeybindings } from '../../keybindings/useKeybinding';
import { type AnalyticsMetadata_I_VERIFIED_THIS_IS_NOT_CODE_OR_FILEPATHS, logEvent } from '../../services/analytics/index';
import { type AppState, useAppState, useSetAppState } from '../../state/AppState';
import type { LocalJSXCommandOnDone } from '../../types/command';
import { clearFastModeCooldown, FAST_MODE_MODEL_DISPLAY, getFastModeModel, getFastModeRuntimeState, getFastModeUnavailableReason, isFastModeEnabled, isFastModeSupportedByModel, prefetchFastModeStatus } from '../../utils/fastMode';
import { formatDuration } from '../../utils/format';
import { formatModelPricing, getOpus46CostTier } from '../../utils/modelCost';
import { updateSettingsForSource } from '../../utils/settings/settings';
function applyFastMode(enable: boolean, setAppState: (f: (prev: AppState) => AppState) => void): void {
  clearFastModeCooldown();
  updateSettingsForSource('userSettings', {
    fastMode: enable ? true : undefined
  });
  if (enable) {
    setAppState(prev => {
      // Only switch model if current model doesn't support fast mode
      const needsModelSwitch = !isFastModeSupportedByModel(prev.mainLoopModel);
      return {
        ...prev,
        ...(needsModelSwitch ? {
          mainLoopModel: getFastModeModel(),
          mainLoopModelForSession: null
        } : {}),
        fastMode: true
      };
    });
  } else {
    setAppState(prev => ({
      ...prev,
      fastMode: false
    }));
  }
}
export function FastModePicker(t0) {
  const {
    onDone,
    unavailableReason
  } = t0;
  const model = useAppState(_temp);
  const initialFastMode = useAppState(_temp2);
  const setAppState = useSetAppState();
  const [enableFastMode, setEnableFastMode] = useState(initialFastMode ?? false);
  const t1 = getFastModeRuntimeState();

  const runtimeState = t1;
  const isCooldown = runtimeState.status === "cooldown";
  const isUnavailable = unavailableReason !== null;
  const t2 = formatModelPricing(getOpus46CostTier(true));

  const pricing = t2;
  const t3 = function handleConfirm() {
      if (isUnavailable) {
        return;
      }
      applyFastMode(enableFastMode, setAppState);
      logEvent("tengu_fast_mode_toggled", {
        enabled: enableFastMode,
        source: "picker" as AnalyticsMetadata_I_VERIFIED_THIS_IS_NOT_CODE_OR_FILEPATHS
      });
      if (enableFastMode) {
        const fastIcon = getFastIconString(enableFastMode);
        const modelUpdated = !isFastModeSupportedByModel(model) ? ` · model set to ${FAST_MODE_MODEL_DISPLAY}` : "";
        onDone(`${fastIcon} Fast mode ON${modelUpdated} · ${pricing}`);
      } else {
        setAppState(_temp3);
        onDone("Fast mode OFF");
      }
    };

  const handleConfirm = t3;
  const t4 = function handleCancel() {
      if (isUnavailable) {
        if (initialFastMode) {
          applyFastMode(false, setAppState);
        }
        onDone("Fast mode OFF", {
          display: "system"
        });
        return;
      }
      const message = initialFastMode ? `${getFastIconString()} Kept Fast mode ON` : "Kept Fast mode OFF";
      onDone(message, {
        display: "system"
      });
    };

  const handleCancel = t4;
  const t5 = function handleToggle() {
      if (isUnavailable) {
        return;
      }
      setEnableFastMode(_temp4);
    };

  const handleToggle = t5;
  const t6 = {
      "confirm:yes": handleConfirm,
      "confirm:nextField": handleToggle,
      "confirm:next": handleToggle,
      "confirm:previous": handleToggle,
      "confirm:cycleMode": handleToggle,
      "confirm:toggle": handleToggle
    };

  const t7 = {
      context: "Confirmation"
    };

  useKeybindings(t6, t7);
  const t8 = <Text><FastIcon cooldown={isCooldown} /> Fast mode (research preview)</Text>;

  const title = t8;
  const t9 = exitState => exitState.pending ? <Text>Press {exitState.keyName} again to exit</Text> : isUnavailable ? <Text>Esc to cancel</Text> : <Text>Tab to toggle · Enter to confirm · Esc to cancel</Text>;

  const t10 = unavailableReason ? <Box marginLeft={2}><Text color="error">{unavailableReason}</Text></Box> : <><Box flexDirection="column" gap={0} marginLeft={2}><Box flexDirection="row" gap={2}><Text bold={true}>Fast mode</Text><Text color={enableFastMode ? "fastMode" : undefined} bold={enableFastMode}>{enableFastMode ? "ON " : "OFF"}</Text><Text dimColor={true}>{pricing}</Text></Box></Box>{isCooldown && runtimeState.status === "cooldown" && <Box marginLeft={2}><Text color="warning">{runtimeState.reason === "overloaded" ? "Fast mode overloaded and is temporarily unavailable" : "You've hit your fast limit"}{" \xB7 resets in "}{formatDuration(runtimeState.resetAt - Date.now(), {
            hideTrailingZeros: true
          })}</Text></Box>}</>;

  const t11 = <Text dimColor={true}>Learn more:{" "}<Link url="https://docs.allternit.com/cli/configuration">https://docs.allternit.com/cli/configuration</Link></Text>;

  const t12 = <Dialog title={title} subtitle={`High-speed mode for ${FAST_MODE_MODEL_DISPLAY}. Billed as extra usage at a premium rate. Separate rate limits apply.`} onCancel={handleCancel} color="fastMode" inputGuide={t9}>{t10}{t11}</Dialog>;

  return t12;
}
function _temp4(prev_0) {
  return !prev_0;
}
function _temp3(prev) {
  return {
    ...prev,
    fastMode: false
  };
}
function _temp2(s_0) {
  return s_0.fastMode;
}
function _temp(s) {
  return s.mainLoopModel;
}
async function handleFastModeShortcut(enable: boolean, getAppState: () => AppState, setAppState: (f: (prev: AppState) => AppState) => void): Promise<string> {
  const unavailableReason = getFastModeUnavailableReason();
  if (unavailableReason) {
    return `Fast mode unavailable: ${unavailableReason}`;
  }
  const {
    mainLoopModel
  } = getAppState();
  applyFastMode(enable, setAppState);
  logEvent('tengu_fast_mode_toggled', {
    enabled: enable,
    source: 'shortcut' as AnalyticsMetadata_I_VERIFIED_THIS_IS_NOT_CODE_OR_FILEPATHS
  });
  if (enable) {
    const fastIcon = getFastIconString(true);
    const modelUpdated = !isFastModeSupportedByModel(mainLoopModel) ? ` · model set to ${FAST_MODE_MODEL_DISPLAY}` : '';
    const pricing = formatModelPricing(getOpus46CostTier(true));
    return `${fastIcon} Fast mode ON${modelUpdated} · ${pricing}`;
  } else {
    return `Fast mode OFF`;
  }
}
export async function call(onDone: LocalJSXCommandOnDone, context: LocalJSXCommandContext, args?: string): Promise<React.ReactNode | null> {
  if (!isFastModeEnabled()) {
    return null;
  }

  // Fetch org fast mode status before showing the picker. We must know
  // whether the org has disabled fast mode before allowing any toggle.
  // If a startup prefetch is already in flight, this awaits it.
  await prefetchFastModeStatus();
  const arg = args?.trim().toLowerCase();
  if (arg === 'on' || arg === 'off') {
    const result = await handleFastModeShortcut(arg === 'on', context.getAppState, context.setAppState);
    onDone(result);
    return null;
  }
  const unavailableReason = getFastModeUnavailableReason();
  logEvent('tengu_fast_mode_picker_shown', {
    unavailable_reason: (unavailableReason ?? '') as AnalyticsMetadata_I_VERIFIED_THIS_IS_NOT_CODE_OR_FILEPATHS
  });
  return <FastModePicker onDone={onDone} unavailableReason={unavailableReason} />;
}
