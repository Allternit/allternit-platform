import * as React from 'react';
import { isPlanView } from './subcommand';
import { handlePlanModeTransition } from '../../bootstrap/state';
import type { LocalJSXCommandContext } from '../../commands';
import { Box, Text } from '../../ink';
import type { LocalJSXCommandOnDone } from '../../types/command';
import { getExternalEditor } from '../../utils/editor';
import { toIDEDisplayName } from '../../utils/ide';
import { applyPermissionUpdate } from '../../utils/permissions/PermissionUpdate';
import { prepareContextForPlanMode } from '../../utils/permissions/permissionSetup';
import { getPlan, getPlanFilePath } from '../../utils/plans';
import { editFileInEditor } from '../../utils/promptEditor';
import { renderToString } from '../../utils/staticRender';
function PlanDisplay(t0) {
  const {
    planContent,
    planPath,
    editorName
  } = t0;
  const t1 = <Text bold={true}>Current Plan</Text>;

  const t2 = <Text dimColor={true}>{planPath}</Text>;

  const t3 = <Box marginTop={1}><Text>{planContent}</Text></Box>;

  const t4 = editorName && <Box marginTop={1}><Text dimColor={true}>"/plan open"</Text><Text dimColor={true}> to edit this plan in </Text><Text bold={true} dimColor={true}>{editorName}</Text></Box>;

  const t5 = <Box flexDirection="column">{t1}{t2}{t3}{t4}</Box>;

  return t5;
}
export async function call(onDone: LocalJSXCommandOnDone, context: LocalJSXCommandContext, args: string): Promise<React.ReactNode> {
  // `/plan view` (was /view-plan): show the saved plan, never enter plan mode.
  if (isPlanView(args)) {
    const { call: viewPlan } = await import('../view-plan/view-plan');
    return viewPlan(onDone, context);
  }
  const {
    getAppState,
    setAppState
  } = context;
  const appState = getAppState();
  const currentMode = appState.toolPermissionContext.mode;

  // If not in plan mode, enable it
  if (currentMode !== 'plan') {
    handlePlanModeTransition(currentMode, 'plan');
    setAppState(prev => ({
      ...prev,
      toolPermissionContext: applyPermissionUpdate(prepareContextForPlanMode(prev.toolPermissionContext), {
        type: 'setMode',
        mode: 'plan',
        destination: 'session'
      })
    }));
    const description = args.trim();
    if (description && description !== 'open') {
      onDone('Enabled plan mode', {
        shouldQuery: true
      });
    } else {
      onDone('Enabled plan mode');
    }
    return null;
  }

  // Already in plan mode - show the current plan
  const planContent = getPlan();
  const planPath = getPlanFilePath();
  if (!planContent) {
    onDone('Already in plan mode. No plan written yet.');
    return null;
  }

  // If user typed "/plan open", open in editor
  const argList = args.trim().split(/\s+/);
  if (argList[0] === 'open') {
    const result = await editFileInEditor(planPath);
    if (result.error) {
      onDone(`Failed to open plan in editor: ${result.error}`);
    } else {
      onDone(`Opened plan in editor: ${planPath}`);
    }
    return null;
  }
  const editor = getExternalEditor();
  const editorName = editor ? toIDEDisplayName(editor) : undefined;
  const display = <PlanDisplay planContent={planContent} planPath={planPath} editorName={editorName} />;

  // Render to string and pass to onDone like local commands do
  const output = await renderToString(display);
  onDone(output);
  return null;
}
