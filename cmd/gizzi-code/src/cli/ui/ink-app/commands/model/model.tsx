import chalk from '@/shared/util/chalk'
import * as React from 'react';
import type { CommandResultDisplay } from '../../commands';
import { ModelPicker } from '../../components/ModelPicker';
import { COMMON_HELP_ARGS, COMMON_INFO_ARGS } from '../../constants/xml';
import { type AnalyticsMetadata_I_VERIFIED_THIS_IS_NOT_CODE_OR_FILEPATHS, logEvent } from '../../services/analytics/index';
import { useAppState, useSetAppState } from '../../state/AppState';
import type { LocalJSXCommandCall } from '../../types/command';
import type { EffortLevel } from '../../utils/effort';
import { isBilledAsExtraUsage } from '../../utils/extraUsage';
import { clearFastModeCooldown, isFastModeAvailable, isFastModeEnabled, isFastModeSupportedByModel } from '../../utils/fastMode';
import { MODEL_ALIASES } from '../../utils/model/aliases';
import { checkOpus1mAccess, checkSonnet1mAccess } from '../../utils/model/check1mAccess';
import { getDefaultMainLoopModelSetting, isOpus1mMergeEnabled, renderDefaultModelSetting } from '../../utils/model/model';
import { isModelAllowed } from '../../utils/model/modelAllowlist';
import { validateModel } from '../../utils/model/validateModel';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { homedir } from 'node:os';
const execFileAsync = promisify(execFile);

// Local providers this machine's ~/mlx-bench/switch-model.sh manages. Only one
// of these servers can run at a time (Qwen ~19GB + Muse ~18GB + Maple ~5-7GB
// together exceed 32GB RAM), so selecting one here must also start its
// server and stop whichever other one is running — otherwise /model just
// changes a config pointer and the next message 503s against a dead port.
const LOCAL_PROVIDER_SWITCH_ARG: Record<string, string> = {
  'local-mlx': 'qwen',
  'muse-glimmer': 'muse',
  'maple-preview': 'maple',
};
const LOCAL_PROVIDER_PORT: Record<string, number> = {
  'local-mlx': 8081,
  'muse-glimmer': 8080,
  'maple-preview': 8082,
};

/**
 * If modelValue belongs to one of the locally-hosted providers above, make
 * sure its server is actually running (starting it via switch-model.sh if
 * needed) before the model selection is considered complete. Returns a short
 * status suffix to append to the "Set model to X" message, or null if this
 * isn't a locally-hosted model or its server was already up.
 */
async function ensureLocalModelServerRunning(modelValue: string | null): Promise<string | null> {
  if (!modelValue) return null;
  const slashIdx = modelValue.indexOf('/');
  if (slashIdx === -1) return null;
  const providerID = modelValue.slice(0, slashIdx);
  const switchArg = LOCAL_PROVIDER_SWITCH_ARG[providerID];
  const port = LOCAL_PROVIDER_PORT[providerID];
  if (!switchArg || !port) return null;

  try {
    const res = await fetch(`http://127.0.0.1:${port}/health`, {
      signal: AbortSignal.timeout(1500),
    });
    if (res.ok) return null; // already running, nothing to do
  } catch {
    // not reachable — fall through and start it
  }

  try {
    await execFileAsync(`${homedir()}/mlx-bench/switch-model.sh`, [switchArg], {
      timeout: 120000,
    });
    return `local server started (${switchArg})`;
  } catch (error) {
    return `WARNING: failed to start local server — ${(error as Error).message}`;
  }
}
function ModelPickerWrapper(t0) {
  const {
    onDone
  } = t0;
  const mainLoopModel = useAppState(_temp);
  const mainLoopModelForSession = useAppState(_temp2);
  const isFastMode = useAppState(_temp3);
  const setAppState = useSetAppState();
  const t1 = function handleCancel() {
      logEvent("tengu_model_command_menu", {
        action: "cancel" as AnalyticsMetadata_I_VERIFIED_THIS_IS_NOT_CODE_OR_FILEPATHS
      });
      const displayModel = renderModelLabel(mainLoopModel);
      onDone(`Kept model as ${chalk.bold(displayModel)}`, {
        display: "system"
      });
    };

  const handleCancel = t1;
  const t2 = async function handleSelect(model, effort) {
      logEvent("tengu_model_command_menu", {
        action: model as AnalyticsMetadata_I_VERIFIED_THIS_IS_NOT_CODE_OR_FILEPATHS,
        from_model: mainLoopModel as AnalyticsMetadata_I_VERIFIED_THIS_IS_NOT_CODE_OR_FILEPATHS,
        to_model: model as AnalyticsMetadata_I_VERIFIED_THIS_IS_NOT_CODE_OR_FILEPATHS
      });
      setAppState(prev => ({
        ...prev,
        mainLoopModel: model,
        mainLoopModelForSession: null
      }));
      const localServerStatus = await ensureLocalModelServerRunning(model);
      let message = `Set model to ${chalk.bold(renderModelLabel(model))}`;
      if (localServerStatus) message = message + ` \xB7 ${localServerStatus}`;
      if (effort !== undefined) {
        message = message + ` with ${chalk.bold(effort)} effort`;
      }
      let wasFastModeToggledOn = undefined;
      if (isFastModeEnabled()) {
        clearFastModeCooldown();
        if (!isFastModeSupportedByModel(model) && isFastMode) {
          setAppState(_temp4);
          wasFastModeToggledOn = false;
        } else {
          if (isFastModeSupportedByModel(model) && isFastModeAvailable() && isFastMode) {
            message = message + " \xB7 Fast mode ON";
            wasFastModeToggledOn = true;
          }
        }
      }
      if (isBilledAsExtraUsage(model, wasFastModeToggledOn === true, isOpus1mMergeEnabled())) {
        message = message + " \xB7 Billed as extra usage";
      }
      if (wasFastModeToggledOn === false) {
        message = message + " \xB7 Fast mode OFF";
      }
      onDone(message);
    };

  const handleSelect = t2;
  const t3 = isFastModeEnabled() && isFastMode && isFastModeSupportedByModel(mainLoopModel) && isFastModeAvailable();

  const t4 = <ModelPicker initial={mainLoopModel} sessionModel={mainLoopModelForSession} onSelect={handleSelect} onCancel={handleCancel} isStandaloneCommand={true} showFastModeNotice={t3} />;

  return t4;
}
function _temp4(prev_0) {
  return {
    ...prev_0,
    fastMode: false
  };
}
function _temp3(s_1) {
  return s_1.fastMode;
}
function _temp2(s_0) {
  return s_0.mainLoopModelForSession;
}
function _temp(s) {
  return s.mainLoopModel;
}
function SetModelAndClose({
  args,
  onDone
}: {
  args: string;
  onDone: (result?: string, options?: {
    display?: CommandResultDisplay;
  }) => void;
}): React.ReactNode {
  const isFastMode = useAppState(s => s.fastMode);
  const setAppState = useSetAppState();
  const model = args === 'default' ? null : args;
  React.useEffect(() => {
    async function handleModelChange(): Promise<void> {
      if (model && !isModelAllowed(model)) {
        onDone(`Model '${model}' is not available. Your organization restricts model selection.`, {
          display: 'system'
        });
        return;
      }

      // @[MODEL LAUNCH]: Update check for 1M access.
      if (model && isOpus1mUnavailable(model)) {
        onDone(`Opus 4.6 with 1M context is not available for your account. Learn more: https://docs.allternit.com/cli/configuration#models-for-each-job`, {
          display: 'system'
        });
        return;
      }
      if (model && isSonnet1mUnavailable(model)) {
        onDone(`Sonnet 4.6 with 1M context is not available for your account. Learn more: https://docs.allternit.com/cli/configuration#models-for-each-job`, {
          display: 'system'
        });
        return;
      }

      // Skip validation for default model
      if (!model) {
        await setModel(null);
        return;
      }

      // Skip validation for known aliases - they're predefined and should work
      if (isKnownAlias(model)) {
        await setModel(model);
        return;
      }

      // Validate and set custom model
      try {
        // Don't use parseUserSpecifiedModel for non-aliases since it lowercases the input
        // and model names are case-sensitive
        const {
          valid,
          error: error_0
        } = await validateModel(model);
        if (valid) {
          await setModel(model);
        } else {
          onDone(error_0 || `Model '${model}' not found`, {
            display: 'system'
          });
        }
      } catch (error) {
        onDone(`Failed to validate model: ${(error as Error).message}`, {
          display: 'system'
        });
      }
    }
    async function setModel(modelValue: string | null): Promise<void> {
      setAppState(prev => ({
        ...prev,
        mainLoopModel: modelValue,
        mainLoopModelForSession: null
      }));
      const localServerStatus = await ensureLocalModelServerRunning(modelValue);
      let message = `Set model to ${chalk.bold(renderModelLabel(modelValue))}`;
      if (localServerStatus) message += ` · ${localServerStatus}`;
      let wasFastModeToggledOn = undefined;
      if (isFastModeEnabled()) {
        clearFastModeCooldown();
        if (!isFastModeSupportedByModel(modelValue) && isFastMode) {
          setAppState(prev_0 => ({
            ...prev_0,
            fastMode: false
          }));
          wasFastModeToggledOn = false;
          // Do not update fast mode in settings since this is an automatic downgrade
        } else if (isFastModeSupportedByModel(modelValue) && isFastMode) {
          message += ` · Fast mode ON`;
          wasFastModeToggledOn = true;
        }
      }
      if (isBilledAsExtraUsage(modelValue, wasFastModeToggledOn === true, isOpus1mMergeEnabled())) {
        message += ` · Billed as extra usage`;
      }
      if (wasFastModeToggledOn === false) {
        // Fast mode was toggled off, show suffix after extra usage billing
        message += ` · Fast mode OFF`;
      }
      onDone(message);
    }
    void handleModelChange();
  }, [model, onDone, setAppState]);
  return null;
}
function isKnownAlias(model: string): boolean {
  return (MODEL_ALIASES as readonly string[]).includes(model.toLowerCase().trim());
}
function isOpus1mUnavailable(model: string): boolean {
  const m = model.toLowerCase();
  return !checkOpus1mAccess() && !isOpus1mMergeEnabled() && m.includes('opus') && m.includes('[1m]');
}
function isSonnet1mUnavailable(model: string): boolean {
  const m = model.toLowerCase();
  // Warn about Sonnet and Sonnet 4.6, but not Sonnet 4.5 since that had
  // a different access criteria.
  return !checkSonnet1mAccess() && (m.includes('sonnet[1m]') || m.includes('sonnet-4-6[1m]'));
}
function ShowModelAndClose(t0) {
  const {
    onDone
  } = t0;
  const mainLoopModel = useAppState(_temp7);
  const mainLoopModelForSession = useAppState(_temp8);
  const effortValue = useAppState(_temp9);
  const displayModel = renderModelLabel(mainLoopModel);
  const effortInfo = effortValue !== undefined ? ` (effort: ${effortValue})` : "";
  if (mainLoopModelForSession) {
    onDone(`Current model: ${chalk.bold(renderModelLabel(mainLoopModelForSession))} (session override from plan mode)\nBase model: ${displayModel}${effortInfo}`);
  } else {
    onDone(`Current model: ${displayModel}${effortInfo}`);
  }
  return null;
}
function _temp9(s_1) {
  return s_1.effortValue;
}
function _temp8(s_0) {
  return s_0.mainLoopModelForSession;
}
function _temp7(s) {
  return s.mainLoopModel;
}
export const call: LocalJSXCommandCall = async (onDone, _context, args) => {
  args = args?.trim() || '';
  if (COMMON_INFO_ARGS.includes(args)) {
    logEvent('tengu_model_command_inline_help', {
      args: args as AnalyticsMetadata_I_VERIFIED_THIS_IS_NOT_CODE_OR_FILEPATHS
    });
    return <ShowModelAndClose onDone={onDone} />;
  }
  if (COMMON_HELP_ARGS.includes(args)) {
    onDone('Run /model to open the model selection menu, or /model [modelName] to set the model.', {
      display: 'system'
    });
    return;
  }
  if (args) {
    logEvent('tengu_model_command_inline', {
      args: args as AnalyticsMetadata_I_VERIFIED_THIS_IS_NOT_CODE_OR_FILEPATHS
    });
    return <SetModelAndClose args={args} onDone={onDone} />;
  }
  return <ModelPickerWrapper onDone={onDone} />;
};
function renderModelLabel(model: string | null): string {
  const rendered = renderDefaultModelSetting(model ?? getDefaultMainLoopModelSetting());
  return model === null ? `${rendered} (default)` : rendered;
}
