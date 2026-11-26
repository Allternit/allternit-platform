import React from 'react';
import { Text } from '../../ink';
import { extractMcpToolDisplayName, getMcpDisplayName } from '../../services/mcp/mcpStringUtils';
import { filterToolsByServer } from '../../services/mcp/utils';
import { useAppState } from '../../state/AppState';
import type { Tool } from '../../Tool';
import { plural } from '../../utils/stringUtils';
import { ConfigurableShortcutHint } from '../ConfigurableShortcutHint';
import { Select } from '../CustomSelect/index';
import { Byline } from '../design-system/Byline';
import { Dialog } from '../design-system/Dialog';
import { KeyboardShortcutHint } from '../design-system/KeyboardShortcutHint';
import type { ServerInfo } from './types';
import { EmptyLine } from '../EmptyLine'
type Props = {
  server: ServerInfo;
  onSelectTool: (tool: Tool, index: number) => void;
  onBack: () => void;
};
export function MCPToolListView({
    server,
    onSelectTool,
    onBack
}: Props) {
  const mcpTools = useAppState(_temp);
  let t1;
  bb0: {
    if (server.client.type !== "connected") {
      const t2 = [];

      t1 = t2;
      break bb0;
    }
    const t2 = filterToolsByServer(mcpTools, server.name);

    t1 = t2;
  }
  const serverTools = t1;
  const t3 = (tool, index) => {
        const toolName = getMcpDisplayName(tool.name, server.name);
        const fullDisplayName = tool.userFacingName ? tool.userFacingName({}) : toolName;
        const displayName = extractMcpToolDisplayName(fullDisplayName);
        const isReadOnly = tool.isReadOnly?.({}) ?? false;
        const isDestructive = tool.isDestructive?.({}) ?? false;
        const isOpenWorld = tool.isOpenWorld?.({}) ?? false;
        const annotations = [];
        if (isReadOnly) {
          annotations.push("read-only");
        }
        if (isDestructive) {
          annotations.push("destructive");
        }
        if (isOpenWorld) {
          annotations.push("open-world");
        }
        return {
          label: displayName,
          value: index.toString(),
          description: annotations.length > 0 ? annotations.join(", ") : undefined,
          descriptionColor: isDestructive ? "error" : isReadOnly ? "success" : undefined
        };
      };

  const t2 = serverTools.map(t3);

  const toolOptions = t2;
  const t3_2 = `Tools for ${server.name}`;
  const t4 = serverTools.length;
  const t5 = plural(serverTools.length, "tool");

  const t6 = `${t4} ${t5}`;
  const t7 = serverTools.length === 0 ? <EmptyLine>No tools available</EmptyLine> : <Select options={toolOptions} onChange={value => {
      const index_0 = parseInt(value);
      const tool_0 = serverTools[index_0];
      if (tool_0) {
        onSelectTool(tool_0, index_0);
      }
    }} onCancel={onBack} />;

  const t8 = <Dialog title={t3_2} subtitle={t6} onCancel={onBack} inputGuide={_temp2}>{t7}</Dialog>;

  return t8;
}
function _temp2(exitState) {
  return exitState.pending ? <Text>Press {exitState.keyName} again to exit</Text> : <Byline><KeyboardShortcutHint shortcut={"\u2191\u2193"} action="navigate" /><KeyboardShortcutHint shortcut="Enter" action="select" /><ConfigurableShortcutHint action="confirm:no" context="Confirmation" fallback="Esc" description="back" /></Byline>;
}
function _temp(s) {
  return s.mcp.tools;
}
