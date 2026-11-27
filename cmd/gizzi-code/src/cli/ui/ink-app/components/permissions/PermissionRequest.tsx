import * as React from 'react';
import { EnterPlanModeTool } from './../../tools/EnterPlanModeTool/EnterPlanModeTool.ts';
import { ExitPlanModeV2Tool } from './../../tools/ExitPlanModeTool/ExitPlanModeV2Tool.ts';
import { useNotifyAfterTimeout } from '../../hooks/useNotifyAfterTimeout';
import { useKeybinding } from '../../keybindings/useKeybinding';
import type { AnyObject, Tool, ToolUseContext } from '../../Tool';
import { AskUserQuestionTool } from '../../tools/AskUserQuestionTool/AskUserQuestionTool';
import { BashTool } from '../../tools/BashTool/BashTool';
import { FileEditTool } from '../../tools/FileEditTool/FileEditTool';
import { FileReadTool } from '../../tools/FileReadTool/FileReadTool';
import { FileWriteTool } from '../../tools/FileWriteTool/FileWriteTool';
import { GlobTool } from '../../tools/GlobTool/GlobTool';
import { GrepTool } from '../../tools/GrepTool/GrepTool';
import { NotebookEditTool } from '../../tools/NotebookEditTool/NotebookEditTool';
import { PowerShellTool } from '../../tools/PowerShellTool/PowerShellTool';
import { SkillTool } from '../../tools/SkillTool/SkillTool';
import { WebFetchTool } from '../../tools/WebFetchTool/WebFetchTool';
import type { AssistantMessage } from '../../types/message';
import type { PermissionDecision } from '../../utils/permissions/PermissionResult';
import { AskUserQuestionPermissionRequest } from './AskUserQuestionPermissionRequest/AskUserQuestionPermissionRequest';
import { BashPermissionRequest } from './BashPermissionRequest/BashPermissionRequest';
import { EnterPlanModePermissionRequest } from './EnterPlanModePermissionRequest/EnterPlanModePermissionRequest';
import { ExitPlanModePermissionRequest } from './ExitPlanModePermissionRequest/ExitPlanModePermissionRequest';
import { FallbackPermissionRequest } from './FallbackPermissionRequest';
import { FileEditPermissionRequest } from './FileEditPermissionRequest/FileEditPermissionRequest';
import { FilesystemPermissionRequest } from './FilesystemPermissionRequest/FilesystemPermissionRequest';
import { FileWritePermissionRequest } from './FileWritePermissionRequest/FileWritePermissionRequest';
import { NotebookEditPermissionRequest } from './NotebookEditPermissionRequest/NotebookEditPermissionRequest';
import { isNeedsYouPermission } from '../../utils/needsYou';
import { NeedsYouPermissionRequest } from './NeedsYouPermissionRequest';
import { PowerShellPermissionRequest } from './PowerShellPermissionRequest/PowerShellPermissionRequest';
import { SkillPermissionRequest } from './SkillPermissionRequest/SkillPermissionRequest';
import { WebFetchPermissionRequest } from './WebFetchPermissionRequest/WebFetchPermissionRequest';

import type { ContentBlockParam } from '@allternit/gizzi-sdk/providers/allternit/resources/messages.mjs';
import type { z } from 'zod/v4';
import type { PermissionUpdate } from '../../utils/permissions/PermissionUpdateSchema';
import type { WorkerBadgeProps } from './WorkerBadge';
function permissionComponentForTool(tool: Tool): React.ComponentType<PermissionRequestProps> {
  if (isNeedsYouPermission(tool.name)) return NeedsYouPermissionRequest;
  switch (tool) {
    case FileEditTool:
      return FileEditPermissionRequest;
    case FileWriteTool:
      return FileWritePermissionRequest;
    case BashTool:
      return BashPermissionRequest;
    case PowerShellTool:
      return PowerShellPermissionRequest;
    case WebFetchTool:
      return WebFetchPermissionRequest;
    case NotebookEditTool:
      return NotebookEditPermissionRequest;
    case ExitPlanModeV2Tool:
      return ExitPlanModePermissionRequest;
    case EnterPlanModeTool:
      return EnterPlanModePermissionRequest;
    case SkillTool:
      return SkillPermissionRequest;
    case AskUserQuestionTool:
      return AskUserQuestionPermissionRequest;
    case GlobTool:
    case GrepTool:
    case FileReadTool:
      return FilesystemPermissionRequest;
    default:
      return FallbackPermissionRequest;
  }
}
export type PermissionRequestProps<Input extends AnyObject = AnyObject> = {
  toolUseConfirm: ToolUseConfirm<Input>;
  toolUseContext: ToolUseContext;
  onDone(): void;
  onReject(): void;
  verbose: boolean;
  workerBadge: WorkerBadgeProps | undefined;
  /**
   * Register JSX to render in a sticky footer below the scrollable area.
   * Fullscreen mode only (non-fullscreen has no sticky area — terminal
   * scrollback moves everything together). Call with null to clear.
   *
   * Used by ExitPlanModePermissionRequest to keep response options visible
   * while the user scrolls through a long plan. The callback is stable —
   * JSX passed should use refs for callbacks that close over component state
   * to avoid stale closures (React reconciles the JSX, preserving Select's
   * internal focus/input state).
   */
  setStickyFooter?: (jsx: React.ReactNode | null) => void;
};
export type ToolUseConfirm<Input extends AnyObject = AnyObject> = {
  assistantMessage: AssistantMessage;
  tool: Tool<Input>;
  description: string;
  input: z.infer<Input>;
  toolUseContext: ToolUseContext;
  toolUseID: string;
  permissionResult: PermissionDecision;
  permissionPromptStartTimeMs: number;
  /**
   * Called when user interacts with the permission dialog (e.g., arrow keys, tab, typing).
   * This prevents async auto-approval mechanisms (like the bash classifier) from
   * dismissing the dialog while the user is actively engaging with it.
   */
  classifierCheckInProgress?: boolean;
  classifierAutoApproved?: boolean;
  classifierMatchedRule?: string;
  workerBadge?: WorkerBadgeProps;
  /**
   * Set when the permission request originates from a dashboard top-level
   * session (stamped on toolUseContext.options by the session's canUseTool
   * wrap). Lets the dashboard render this confirm inline, attributed to the
   * right row. Absent = main session's own prompt.
   */
  dashboardTaskId?: string;
  onUserInteraction(): void;
  onAbort(): void;
  onDismissCheckmark?(): void;
  onAllow(updatedInput: z.infer<Input>, permissionUpdates: PermissionUpdate[], feedback?: string, contentBlocks?: ContentBlockParam[]): void;
  onReject(feedback?: string, contentBlocks?: ContentBlockParam[]): void;
  /** Agent-browser needs-you prompts: the button pressed ("task" = the second button) and a masked answer. */
  onNeedsYou?(reply: 'once' | 'task', answer?: string): void;
  recheckPermission(): Promise<void>;
};
function getNotificationMessage(toolUseConfirm: ToolUseConfirm): string {
  const toolName = toolUseConfirm.tool.userFacingName(toolUseConfirm.input as never);
  if (toolUseConfirm.tool === ExitPlanModeV2Tool) {
    return 'Gizzi Code needs your approval for the plan';
  }
  if (toolUseConfirm.tool === EnterPlanModeTool) {
    return 'Gizzi Code wants to enter plan mode';
  }
  if (!toolName || toolName.trim() === '') {
    return 'Gizzi Code needs your attention';
  }
  return `Gizzi needs your permission to use ${toolName}`;
}

// TODO: Move this to Tool.renderPermissionRequest
export function PermissionRequest(t0) {
  const {
    toolUseConfirm,
    toolUseContext,
    onDone,
    onReject,
    verbose,
    workerBadge,
    setStickyFooter
  } = t0;
  const t1 = () => {
      onDone();
      onReject();
      toolUseConfirm.onReject();
    };

  const t2 = {
      context: "Confirmation"
    };

  useKeybinding("app:interrupt", t1, t2);
  const t3 = getNotificationMessage(toolUseConfirm);

  const notificationMessage = t3;
  useNotifyAfterTimeout(notificationMessage, "permission_prompt");
  const t4 = permissionComponentForTool(toolUseConfirm.tool);

  const PermissionComponent = t4;
  const t5 = <PermissionComponent toolUseContext={toolUseContext} toolUseConfirm={toolUseConfirm} onDone={onDone} onReject={onReject} verbose={verbose} workerBadge={workerBadge} setStickyFooter={setStickyFooter} />;

  return t5;
}
