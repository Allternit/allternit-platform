import figures from 'figures';
import * as React from 'react';
import { Suspense, use } from 'react';
import { getSessionId } from '../../bootstrap/state';
import type { LocalJSXCommandContext } from '../../commands';
import { useIsInsideModal } from '../../context/modalContext';
import { Box, Text, useInput, useTheme } from '../../ink';
import { setClipboard } from '../../ink/termio/osc';
import { type AppState, useAppState } from '../../state/AppState';
import { getCwd } from '../../utils/cwd';
import { getCurrentSessionTitle } from '../../utils/sessionStorage';
import { buildAccountProperties, buildAPIProviderProperties, buildIDEProperties, buildInstallationDiagnostics, buildInstallationHealthDiagnostics, buildMcpProperties, buildMemoryDiagnostics, buildSandboxProperties, buildSettingSourcesProperties, type Diagnostic, getModelDisplayLabel, type Property } from '../../utils/status';
import { countUserTurns, formatServicesLine, getAuthMethodDescription, getServicesLine } from '../../utils/statusModel';
import type { ThemeName } from '../../utils/theme';
import { ConfigurableShortcutHint } from '../ConfigurableShortcutHint';
type Props = {
  context: LocalJSXCommandContext;
  diagnosticsPromise: Promise<Diagnostic[]>;
  isActiveTab?: boolean;
};
async function copyToClipboard(text: string): Promise<void> {
  const raw = await setClipboard(text);
  if (raw) process.stdout.write(raw);
}
function propertiesToText(properties: Property[]): string {
  const lines: string[] = [];
  for (const p of properties) {
    if (p.label === undefined) {
      continue;
    }
    let value = '';
    if (Array.isArray(p.value)) {
      value = p.value.join(', ');
    } else if (typeof p.value === 'string' || typeof p.value === 'number') {
      value = String(p.value);
    }
    lines.push(`${p.label}: ${value}`.trimEnd());
  }
  return lines.join('\n');
}
function buildPrimarySection(context: LocalJSXCommandContext): Property[] {
  const sessionId = getSessionId();
  const customTitle = getCurrentSessionTitle(sessionId);
  const nameValue = customTitle ?? <Text dimColor>/rename to add a name</Text>;
  const turns = countUserTurns((context.getAppState() as { messages?: unknown[] }).messages);
  const accountProperties = buildAccountProperties();
  const authFallback = accountProperties.length === 0 && getAuthMethodDescription() !== 'none' ? [{
    label: 'Auth method',
    value: getAuthMethodDescription()
  }] : [];
  return [{
    label: 'Version',
    value: MACRO.VERSION
  }, {
    label: 'Session name',
    value: nameValue
  }, {
    label: 'Session ID',
    value: sessionId
  }, {
    label: 'cwd',
    value: getCwd()
  }, {
    label: 'Turns',
    value: String(turns)
  }, ...accountProperties, ...authFallback, ...buildAPIProviderProperties()];
}
function buildSecondarySection({
  mainLoopModel,
  mcp,
  theme,
  context,
  services
}: {
  mainLoopModel: AppState['mainLoopModel'];
  mcp: AppState['mcp'];
  theme: ThemeName;
  context: LocalJSXCommandContext;
  services?: string | null;
}): Property[] {
  const modelLabel = getModelDisplayLabel(mainLoopModel);
  return [{
    label: 'Model',
    value: modelLabel
  }, ...buildIDEProperties(mcp.clients, context.options.ideInstallationStatus, theme), ...buildMcpProperties(mcp.clients, theme), ...buildSandboxProperties(), ...buildSettingSourcesProperties(), ...(services ? [{
    label: 'Services',
    value: services
  }] : [])];
}
export async function buildDiagnostics(): Promise<Diagnostic[]> {
  return [...(await buildInstallationDiagnostics()), ...(await buildInstallationHealthDiagnostics()), ...(await buildMemoryDiagnostics())];
}
function PropertyValue(t0) {
  const {
    value
  } = t0;
  if (Array.isArray(value)) {
    const t2 = (item, i) => <Text key={i}>{item}{i < value.length - 1 ? "," : ""}</Text>;

    const t1 = value.map(t2);

    const t2_2 = <Box flexWrap="wrap" columnGap={1} flexShrink={99}>{t1}</Box>;

    return t2_2;
  }
  if (typeof value === "string") {
    const t1 = <Text>{value}</Text>;

    return t1;
  }
  return value;
}
export function Status({
    context,
    diagnosticsPromise,
    isActiveTab
}: Props) {
  const tabActive = isActiveTab !== false;
  const [copiedHint, setCopiedHint] = React.useState(null);
  const mainLoopModel = useAppState(_temp);
  const mcp = useAppState(_temp2);
  const [theme] = useTheme();
  const [services, setServices] = React.useState<string | null>(null);
  React.useEffect(() => {
    let live = true;
    void getServicesLine().then(l => {
      if (live) setServices(formatServicesLine(l));
    });
    return () => {
      live = false;
    };
  }, []);
  useInput((input, _key, event) => {
    if (input === 'c') {
      event.stopImmediatePropagation();
      void copyToClipboard(getSessionId());
      setCopiedHint('Session ID copied to clipboard');
      setTimeout(() => setCopiedHint(null), 2000);
    } else if (input === 'y') {
      event.stopImmediatePropagation();
      const text = sections.map(propertiesToText).filter(Boolean).join('\n');
      void copyToClipboard(text);
      setCopiedHint('Session info copied to clipboard');
      setTimeout(() => setCopiedHint(null), 2000);
    }
  }, {
    isActive: tabActive
  });
  const t1 = buildPrimarySection(context);

  const t2 = buildSecondarySection({
      mainLoopModel,
      mcp,
      theme,
      context,
      services
    });

  const t3 = [t1, t2];

  const sections = t3;
  const grow = useIsInsideModal() ? 1 : undefined;
  const t4 = sections.map(_temp4);

  const t5 = <Suspense fallback={null}><Diagnostics promise={diagnosticsPromise} /></Suspense>;

  const t6 = <Box flexDirection="column" gap={1} flexGrow={grow}>{t4}{t5}</Box>;

  const t7 = <Text dimColor={true}><ConfigurableShortcutHint action="confirm:no" context="Settings" fallback="Esc" description="cancel" /></Text>;

  const hintText = copiedHint ?? 'c copy session id · y copy all · click Session ID to copy';
  const t9 = <Box flexDirection="row" gap={2}><Text dimColor={copiedHint === null} color={copiedHint !== null ? "#d4b08c" : undefined}>{hintText}</Text></Box>;

  const t8 = <Box flexDirection="column" flexGrow={grow}>{t6}{t7}{t9}</Box>;

  return t8;
}
function _temp4(properties, i) {
  return properties.length > 0 && <Box key={i} flexDirection="column">{properties.map(_temp3)}</Box>;
}
function _temp3(t0, j) {
  const {
    label,
    value
  } = t0;
  if (label === 'Session ID') {
    return <Box key={j} flexDirection="row" gap={1} flexShrink={0} onClick={() => {
      void copyToClipboard(getSessionId());
    }}><Text bold={true}>{label}:</Text><PropertyValue value={value} /></Box>;
  }
  return <Box key={j} flexDirection="row" gap={1} flexShrink={0}>{label !== undefined && <Text bold={true}>{label}:</Text>}<PropertyValue value={value} /></Box>;
}
function _temp2(s_0) {
  return s_0.mcp;
}
function _temp(s) {
  return s.mainLoopModel;
}
function Diagnostics({
  promise
}: {
  promise: Promise<Diagnostic[]>;
}) {
  const diagnostics = use(promise);
  if (diagnostics.length === 0) {
    return null;
  }
  const t1 = <Text bold={true}>System Diagnostics</Text>;

  const t2 = diagnostics.map(_temp5);

  const t3 = <Box flexDirection="column" paddingBottom={1}>{t1}{t2}</Box>;

  return t3;
}
function _temp5(diagnostic, i) {
  return <Box key={i} flexDirection="row" gap={1} paddingX={1}><Text color="error">{figures.warning}</Text>{typeof diagnostic === "string" ? <Text wrap="wrap">{diagnostic}</Text> : diagnostic}</Box>;
}
