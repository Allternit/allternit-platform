import figures from 'figures';
import { join } from 'path';
import React, { Suspense, use, useCallback, useEffect, useMemo, useState } from 'react';
import { KeybindingWarnings } from './../components/KeybindingWarnings.tsx';
import { McpParsingWarnings } from './../components/mcp/McpParsingWarnings.tsx';
import { getModelMaxOutputTokens } from './../utils/context.ts';
import { getGizziConfigHomeDir } from './../utils/envUtils.ts';
import type { SettingSource } from './../utils/settings/constants.ts';
import { getOriginalCwd } from '../bootstrap/state';
import type { CommandResultDisplay } from '../commands';
import { Pane } from '../components/design-system/Pane';
import { PressEnterToContinue } from '../components/PressEnterToContinue';
import { SandboxDoctorSection } from '../components/sandbox/SandboxDoctorSection';
import { PluginDirsDoctorSection } from '../components/PluginDirsDoctorSection';
import { ValidationErrorsList } from '../components/ValidationErrorsList';
import { useSettingsErrors } from '../hooks/notifs/useSettingsErrors';
import { useExitOnCtrlCDWithKeybindings } from '../hooks/useExitOnCtrlCDWithKeybindings';
import { Box, Text } from '../ink';
import { useKeybindings } from '../keybindings/useKeybinding';
import { useAppState } from '../state/AppState';
import { getPluginErrorMessage } from '../types/plugin';
import type { NpmDistTags } from '../utils/autoUpdater';
import { Updater } from '@/shared/installation/updater';
import { type ContextWarnings, checkContextWarnings } from '../utils/doctorContextWarnings';
import { type DiagnosticInfo, getDoctorDiagnostic } from '../utils/doctorDiagnostic';
import { validateBoundedIntEnvVar } from '../utils/envValidation';
import { pathExists } from '../utils/file';
import { cleanupStaleLocks, getAllLockInfo, isPidBasedLockingEnabled, type LockInfo } from '../utils/nativeInstaller/pidLock';
import { getInitialSettings } from '../utils/settings/settings';
import { BASH_MAX_OUTPUT_DEFAULT, BASH_MAX_OUTPUT_UPPER_LIMIT } from '../utils/shell/outputLimits';
import { TASK_MAX_OUTPUT_DEFAULT, TASK_MAX_OUTPUT_UPPER_LIMIT } from '../utils/task/outputFormatting';
import { getXDGStateHome } from '../utils/xdg';
type Props = {
  onDone: (result?: string, options?: {
    display?: CommandResultDisplay;
  }) => void;
};
type AgentInfo = {
  activeAgents: Array<{
    agentType: string;
    source: SettingSource | 'built-in' | 'plugin';
  }>;
  userAgentsDir: string;
  projectAgentsDir: string;
  userDirExists: boolean;
  projectDirExists: boolean;
  failedFiles?: Array<{
    path: string;
    error: string;
  }>;
};
type VersionLockInfo = {
  enabled: boolean;
  locks: LockInfo[];
  locksDir: string;
  staleLocksCleaned: number;
};
function DistTagsDisplay(t0: {
  promise: Promise<NpmDistTags>;
}) {
  const {
    promise
  } = t0;
  const distTags = use(promise);
  if (!distTags.latest) {
    const t1 = <Text dimColor={true}>└ Latest version: not checked yet</Text>;

    return t1;
  }
  const t1 = distTags.stable && <Text>└ Stable version: {distTags.stable}</Text>;

  const t2 = <Text>└ Latest version: {distTags.latest}</Text>;

  const t3 = <>{t1}{t2}</>;

  return t3;
}
export function Doctor({
    onDone
}: Props) {
  const agentDefinitions = useAppState(_temp);
  const mcpTools = useAppState(_temp2);
  const toolPermissionContext = useAppState(_temp3);
  const pluginsErrors = useAppState(_temp4);
  useExitOnCtrlCDWithKeybindings();
  const t1 = mcpTools || [];

  const tools = t1;
  const [diagnostic, setDiagnostic] = useState(null);
  const [agentInfo, setAgentInfo] = useState(null);
  const [contextWarnings, setContextWarnings] = useState(null);
  const [versionLockInfo, setVersionLockInfo] = useState(null);
  const validationErrors = useSettingsErrors();
  const t2 = getDoctorDiagnostic().then(_temp6);

  const distTagsPromise = t2;
  const autoUpdatesChannel = getInitialSettings()?.autoUpdatesChannel ?? "latest";
  const t3 = validationErrors.filter(_temp7);

  const errorsExcludingMcp = t3;
  const envVars = [{
      name: "BASH_MAX_OUTPUT_LENGTH",
      default: BASH_MAX_OUTPUT_DEFAULT,
      upperLimit: BASH_MAX_OUTPUT_UPPER_LIMIT
    }, {
      name: "TASK_MAX_OUTPUT_LENGTH",
      default: TASK_MAX_OUTPUT_DEFAULT,
      upperLimit: TASK_MAX_OUTPUT_UPPER_LIMIT
    }, {
      name: "GIZZI_CODE_MAX_OUTPUT_TOKENS",
      ...getModelMaxOutputTokens("claude-opus-4-6")
    }];
  const t4 = envVars.map(_temp8).filter(_temp9);

  const envValidationErrors = t4;
  const t5 = () => {
      getDoctorDiagnostic().then(setDiagnostic);
      (async () => {
        const userAgentsDir = join(getGizziConfigHomeDir(), "agents");
        const projectAgentsDir = join(getOriginalCwd(), ".claude", "agents");
        const {
          activeAgents,
          allAgents,
          failedFiles
        } = agentDefinitions;
        const [userDirExists, projectDirExists] = await Promise.all([pathExists(userAgentsDir), pathExists(projectAgentsDir)]);
        const agentInfoData = {
          activeAgents: activeAgents.map(_temp0),
          userAgentsDir,
          projectAgentsDir,
          userDirExists,
          projectDirExists,
          failedFiles
        };
        setAgentInfo(agentInfoData);
        const warnings = await checkContextWarnings(tools, {
          activeAgents,
          allAgents,
          failedFiles
        }, async () => toolPermissionContext);
        setContextWarnings(warnings);
        if (isPidBasedLockingEnabled()) {
          const locksDir = join(getXDGStateHome(), "claude", "locks");
          const staleLocksCleaned = cleanupStaleLocks(locksDir);
          const locks = getAllLockInfo(locksDir);
          setVersionLockInfo({
            enabled: true,
            locks,
            locksDir,
            staleLocksCleaned
          });
        } else {
          setVersionLockInfo({
            enabled: false,
            locks: [],
            locksDir: "",
            staleLocksCleaned: 0
          });
        }
      })();
    };
  const t6 = [toolPermissionContext, tools, agentDefinitions];

  useEffect(t5, t6);
  const t7 = () => {
      onDone("Gizzi Code diagnostics dismissed", {
        display: "system"
      });
    };

  const handleDismiss = t7;
  const t8 = {
      "confirm:yes": handleDismiss,
      "confirm:no": handleDismiss
    };

  const t9 = {
      context: "Confirmation"
    };

  useKeybindings(t8, t9);
  if (!diagnostic) {
    const t10 = <Pane><Text dimColor={true}>Checking installation status…</Text></Pane>;

    return t10;
  }
  const t10 = <Text bold={true}>Diagnostics</Text>;

  const t11 = <Text>└ Currently running: {diagnostic.installationType} ({diagnostic.version})</Text>;

  const t12 = diagnostic.packageManager && <Text>└ Package manager: {diagnostic.packageManager}</Text>;

  const t13 = <Text>└ Path: {diagnostic.installationPath}</Text>;

  const t14 = <Text>└ Invoked: {diagnostic.invokedBinary}</Text>;

  const t15 = <Text>└ Config install method: {diagnostic.configInstallMethod}</Text>;

  const t16 = diagnostic.ripgrepStatus.working ? "OK" : "Not working";
  const t17 = diagnostic.ripgrepStatus.mode === "embedded" ? "bundled" : diagnostic.ripgrepStatus.mode === "builtin" ? "vendor" : diagnostic.ripgrepStatus.systemPath || "system";
  const t18 = <Text>└ Search: {t16} ({t17})</Text>;

  const t19 = diagnostic.recommendation && <><Text /><Text color="warning">Recommendation: {diagnostic.recommendation.split("\n")[0]}</Text><Text dimColor={true}>{diagnostic.recommendation.split("\n")[1]}</Text></>;

  const t20 = diagnostic.multipleInstallations.length > 1 && <><Text /><Text color="warning">Warning: Multiple installations found</Text>{diagnostic.multipleInstallations.map(_temp1)}</>;

  const t21 = diagnostic.warnings.length > 0 && <><Text />{diagnostic.warnings.map(_temp10)}</>;

  const t22 = errorsExcludingMcp.length > 0 && <Box flexDirection="column" marginTop={1} marginBottom={1}><Text bold={true}>Invalid Settings</Text><ValidationErrorsList errors={errorsExcludingMcp} /></Box>;

  const t23 = <Box flexDirection="column">{t10}{t11}{t12}{t13}{t14}{t15}{t18}{t19}{t20}{t21}{t22}</Box>;

  const t24 = <Text bold={true}>Updates</Text>;

  const t25 = diagnostic.packageManager ? "Managed by package manager" : diagnostic.autoUpdates;
  const t26 = <Text>└ Auto-updates:{" "}{t25}</Text>;

  const t27 = diagnostic.hasUpdatePermissions !== null && <Text>└ Update permissions:{" "}{diagnostic.hasUpdatePermissions ? "Yes" : "No (requires sudo)"}</Text>;

  const t28 = <Text>└ Auto-update channel: {autoUpdatesChannel}</Text>;

  const t29 = <Suspense fallback={null}><DistTagsDisplay promise={distTagsPromise} /></Suspense>;

  const t30 = <Box flexDirection="column">{t24}{t26}{t27}{t28}{t29}</Box>;

  const t31 = <><SandboxDoctorSection /><PluginDirsDoctorSection /></>;
  const t32 = <McpParsingWarnings />;
  const t33 = <KeybindingWarnings />;
  const t34 = envValidationErrors.length > 0 && <Box flexDirection="column"><Text bold={true}>Environment Variables</Text>{envValidationErrors.map(_temp11)}</Box>;

  const t35 = versionLockInfo?.enabled && <Box flexDirection="column"><Text bold={true}>Version Locks</Text>{versionLockInfo.staleLocksCleaned > 0 && <Text dimColor={true}>└ Cleaned {versionLockInfo.staleLocksCleaned} stale lock(s)</Text>}{versionLockInfo.locks.length === 0 ? <Text dimColor={true}>└ No active version locks</Text> : versionLockInfo.locks.map(_temp12)}</Box>;

  const t36 = agentInfo?.failedFiles && agentInfo.failedFiles.length > 0 && <Box flexDirection="column"><Text bold={true} color="error">Agent Parse Errors</Text><Text color="error">└ Failed to parse {agentInfo.failedFiles.length} agent file(s):</Text>{agentInfo.failedFiles.map(_temp13)}</Box>;

  const t37 = pluginsErrors.length > 0 && <Box flexDirection="column"><Text bold={true} color="error">Plugin Errors</Text><Text color="error">└ {pluginsErrors.length} plugin error(s) detected:</Text>{pluginsErrors.map(_temp14)}</Box>;

  const t38 = contextWarnings?.unreachableRulesWarning && <Box flexDirection="column"><Text bold={true} color="warning">Unreachable Permission Rules</Text><Text>└{" "}<Text color="warning">{figures.warning}{" "}{contextWarnings.unreachableRulesWarning.message}</Text></Text>{contextWarnings.unreachableRulesWarning.details.map(_temp15)}</Box>;

  const t39 = contextWarnings && (contextWarnings.claudeMdWarning || contextWarnings.agentWarning || contextWarnings.mcpWarning) && <Box flexDirection="column"><Text bold={true}>Context Usage Warnings</Text>{contextWarnings.claudeMdWarning && <><Text>└{" "}<Text color="warning">{figures.warning} {contextWarnings.claudeMdWarning.message}</Text></Text><Text>{"  "}└ Files:</Text>{contextWarnings.claudeMdWarning.details.map(_temp16)}</>}{contextWarnings.agentWarning && <><Text>└{" "}<Text color="warning">{figures.warning} {contextWarnings.agentWarning.message}</Text></Text><Text>{"  "}└ Top contributors:</Text>{contextWarnings.agentWarning.details.map(_temp17)}</>}{contextWarnings.mcpWarning && <><Text>└{" "}<Text color="warning">{figures.warning} {contextWarnings.mcpWarning.message}</Text></Text><Text>{"  "}└ MCP servers:</Text>{contextWarnings.mcpWarning.details.map(_temp18)}</>}</Box>;

  const t40 = <Box><PressEnterToContinue /></Box>;

  const t41 = <Pane>{t23}{t30}{t31}{t32}{t33}{t34}{t35}{t36}{t37}{t38}{t39}{t40}</Pane>;

  return t41;
}
function _temp18(detail_2, i_8) {
  return <Text key={i_8} dimColor={true}>{"    "}└ {detail_2}</Text>;
}
function _temp17(detail_1, i_7) {
  return <Text key={i_7} dimColor={true}>{"    "}└ {detail_1}</Text>;
}
function _temp16(detail_0, i_6) {
  return <Text key={i_6} dimColor={true}>{"    "}└ {detail_0}</Text>;
}
function _temp15(detail, i_5) {
  return <Text key={i_5} dimColor={true}>{"  "}└ {detail}</Text>;
}
function _temp14(error_0, i_4) {
  return <Text key={i_4} dimColor={true}>{"  "}└ {error_0.source || "unknown"}{"plugin" in error_0 && error_0.plugin ? ` [${error_0.plugin}]` : ""}:{" "}{getPluginErrorMessage(error_0)}</Text>;
}
function _temp13(file, i_3) {
  return <Text key={i_3} dimColor={true}>{"  "}└ {file.path}: {file.error}</Text>;
}
function _temp12(lock, i_2) {
  return <Text key={i_2}>└ {lock.version}: PID {lock.pid}{" "}{lock.isProcessRunning ? <Text>(running)</Text> : <Text color="warning">(stale)</Text>}</Text>;
}
function _temp11(validation, i_1) {
  return <Text key={i_1}>└ {validation.name}:{" "}<Text color={validation.status === "capped" ? "warning" : "error"}>{validation.message}</Text></Text>;
}
function _temp10(warning, i_0) {
  return <Box key={i_0} flexDirection="column"><Text color="warning">Warning: {warning.issue}</Text><Text>Fix: {warning.fix}</Text></Box>;
}
function _temp1(install, i) {
  return <Text key={i}>└ {install.type} at {install.path}</Text>;
}
function _temp0(a) {
  return {
    agentType: a.agentType,
    source: a.source
  };
}
function _temp9(v_0) {
  return v_0.status !== "valid";
}
function _temp8(v) {
  const value = process.env[v.name];
  const result = validateBoundedIntEnvVar(v.name, value, v.default, v.upperLimit);
  return {
    name: v.name,
    ...result
  };
}
function _temp7(error) {
  return error.mcpErrorMetadata === undefined;
}
function _temp6() {
  // The signed release feed is the only version source (PLAN.md D2); the
  // background updater has already read it, so doctor never fetches here.
  const status = Updater.getStatus();
  return {
    latest: status.latest ?? null,
    stable: null
  };
}
function _temp4(s_2) {
  return s_2.plugins.errors;
}
function _temp3(s_1) {
  return s_1.toolPermissionContext;
}
function _temp2(s_0) {
  return s_0.mcp.tools;
}
function _temp(s) {
  return s.agentDefinitions;
}
