import figures from 'figures';
import React, { useCallback, useState } from 'react';
import type { CommandResultDisplay } from '../../commands';
import { Box, color, Link, Text, useTheme } from '../../ink';
import { useKeybindings } from '../../keybindings/useKeybinding';
import type { ConfigScope } from '../../services/mcp/types';
import { describeMcpConfigFilePath } from '../../services/mcp/utils';
import { isDebugMode } from '../../utils/debug';
import { plural } from '../../utils/stringUtils';
import { ConfigurableShortcutHint } from '../ConfigurableShortcutHint';
import { Byline } from '../design-system/Byline';
import { Dialog } from '../design-system/Dialog';
import { KeyboardShortcutHint } from '../design-system/KeyboardShortcutHint';
import { McpParsingWarnings } from './McpParsingWarnings';
import type { AgentMcpServerInfo, ServerInfo } from './types';
type Props = {
  servers: ServerInfo[];
  agentServers?: AgentMcpServerInfo[];
  onSelectServer: (server: ServerInfo) => void;
  onSelectAgentServer?: (agentServer: AgentMcpServerInfo) => void;
  onComplete: (result?: string, options?: {
    display?: CommandResultDisplay;
  }) => void;
  defaultTab?: string;
};
type SelectableItem = {
  type: 'server';
  server: ServerInfo;
} | {
  type: 'agent-server';
  agentServer: AgentMcpServerInfo;
};

// Define scope order for display (constant, outside component)
// 'dynamic' (built-in) is rendered separately at the end
const SCOPE_ORDER: ConfigScope[] = ['project', 'local', 'user', 'enterprise'];

// Get scope heading parts (label is bold, path is grey)
function getScopeHeading(scope: ConfigScope): {
  label: string;
  path?: string;
} {
  switch (scope) {
    case 'project':
      return {
        label: 'Project MCPs',
        path: describeMcpConfigFilePath(scope)
      };
    case 'user':
      return {
        label: 'User MCPs',
        path: describeMcpConfigFilePath(scope)
      };
    case 'local':
      return {
        label: 'Local MCPs',
        path: describeMcpConfigFilePath(scope)
      };
    case 'enterprise':
      return {
        label: 'Enterprise MCPs'
      };
    case 'dynamic':
      return {
        label: 'Built-in MCPs',
        path: 'always available'
      };
    default:
      return {
        label: scope
      };
  }
}

// Group servers by scope
function groupServersByScope(serverList: ServerInfo[]): Map<ConfigScope, ServerInfo[]> {
  const groups = new Map<ConfigScope, ServerInfo[]>();
  for (const server of serverList) {
    const scope = server.scope;
    if (!groups.has(scope)) {
      groups.set(scope, []);
    }
    groups.get(scope)!.push(server);
  }
  // Sort servers within each group alphabetically
  for (const [, groupServers] of groups) {
    groupServers.sort((a, b) => a.name.localeCompare(b.name));
  }
  return groups;
}
export function MCPListPanel({
    servers,
    agentServers: t1,
    onSelectServer,
    onSelectAgentServer,
    onComplete
}: Props) {
  const t2 = t1 === undefined ? [] : t1;

  const agentServers = t2;
  const [theme] = useTheme();
  const [selectedIndex, setSelectedIndex] = useState(0);
  const regularServers = servers.filter(_temp);
  const t3 = groupServersByScope(regularServers);

  const serversByScope = t3;
  const t4 = servers.filter(_temp2).sort(_temp3);

  const claudeAiServers = t4;
  const t5 = (serversByScope.get("dynamic") ?? []).sort(_temp4);

  const dynamicServers = t5;
  const t6 = getScopeHeading("dynamic");

  const dynamicHeading = t6;
  const items = [];
  for (const scope of SCOPE_ORDER) {
      const scopeServers = serversByScope.get(scope) ?? [];
      for (const server of scopeServers) {
        items.push({
          type: "server",
          server
        });
      }
    }
  for (const server_0 of claudeAiServers) {
      items.push({
        type: "server",
        server: server_0
      });
    }
  for (const agentServer of agentServers) {
      items.push({
        type: "agent-server",
        agentServer
      });
    }
  for (const server_1 of dynamicServers) {
      items.push({
        type: "server",
        server: server_1
      });
    }

  const selectableItems = items;
  const t7 = () => {
      onComplete("MCP dialog dismissed", {
        display: "system"
      });
    };

  const handleCancel = t7;
  const t8 = () => {
      const item = selectableItems[selectedIndex];
      if (!item) {
        return;
      }
      if (item.type === "server") {
        onSelectServer(item.server);
      } else {
        if (item.type === "agent-server" && onSelectAgentServer) {
          onSelectAgentServer(item.agentServer);
        }
      }
    };

  const handleSelect = t8;
  const t9 = () => setSelectedIndex(prev => prev === 0 ? selectableItems.length - 1 : prev - 1);
  const t10 = () => setSelectedIndex(prev_0 => prev_0 === selectableItems.length - 1 ? 0 : prev_0 + 1);

  const t11 = {
      "confirm:previous": t9,
      "confirm:next": t10,
      "confirm:yes": handleSelect,
      "confirm:no": handleCancel
    };

  const t12 = {
      context: "Confirmation"
    };

  useKeybindings(t11, t12);
  const t13 = server_2 => selectableItems.findIndex(item_0 => item_0.type === "server" && item_0.server === server_2);

  const getServerIndex = t13;
  const t14 = agentServer_0 => selectableItems.findIndex(item_1 => item_1.type === "agent-server" && item_1.agentServer === agentServer_0);

  const getAgentServerIndex = t14;
  const t15 = isDebugMode();

  const debugMode = t15;
  const t16 = servers.some(_temp5);

  const hasFailedClients = t16;
  if (servers.length === 0 && agentServers.length === 0) {
    return null;
  }
  const t17 = server_3 => {
      const index = getServerIndex(server_3);
      const isSelected = selectedIndex === index;
      let statusIcon;
      let statusText;
      if (server_3.client.type === "disabled") {
        statusIcon = color("inactive", theme)(figures.radioOff);
        statusText = "disabled";
      } else {
        if (server_3.client.type === "connected") {
          statusIcon = color("success", theme)(figures.tick);
          statusText = "connected";
        } else {
          if (server_3.client.type === "pending") {
            statusIcon = color("inactive", theme)(figures.radioOff);
            const {
              reconnectAttempt,
              maxReconnectAttempts
            } = server_3.client;
            if (reconnectAttempt && maxReconnectAttempts) {
              statusText = `reconnecting (${reconnectAttempt}/${maxReconnectAttempts})…`;
            } else {
              statusText = "connecting\u2026";
            }
          } else {
            if (server_3.client.type === "needs-auth") {
              // @ts-expect-error TODO(types) vendored figures@3 lacks upstream 'triangleUpOutline'
              statusIcon = color("warning", theme)(figures.triangleUpOutline);
              statusText = "needs authentication";
            } else {
              statusIcon = color("error", theme)(figures.cross);
              statusText = "failed";
            }
          }
        }
      }
      return <Box key={`${server_3.name}-${index}`}><Text color={isSelected ? "suggestion" : undefined}>{isSelected ? `${figures.pointer} ` : "  "}</Text><Text color={isSelected ? "suggestion" : undefined}>{server_3.name}</Text><Text dimColor={!isSelected}> · {statusIcon} </Text><Text dimColor={!isSelected}>{statusText}</Text></Box>;
    };

  const renderServerItem = t17;
  const t18 = agentServer_1 => {
      const index_0 = getAgentServerIndex(agentServer_1);
      const isSelected_0 = selectedIndex === index_0;
      // @ts-expect-error TODO(types) vendored figures@3 lacks upstream 'triangleUpOutline'
      const statusIcon_0 = agentServer_1.needsAuth ? color("warning", theme)(figures.triangleUpOutline) : color("inactive", theme)(figures.radioOff);
      const statusText_0 = agentServer_1.needsAuth ? "may need auth" : "agent-only";
      return <Box key={`agent-${agentServer_1.name}-${index_0}`}><Text color={isSelected_0 ? "suggestion" : undefined}>{isSelected_0 ? `${figures.pointer} ` : "  "}</Text><Text color={isSelected_0 ? "suggestion" : undefined}>{agentServer_1.name}</Text><Text dimColor={!isSelected_0}> · {statusIcon_0} </Text><Text dimColor={!isSelected_0}>{statusText_0}</Text></Box>;
    };

  const renderAgentServerItem = t18;
  const totalServers = servers.length + agentServers.length;
  const t19 = <McpParsingWarnings />;

  const t20 = plural(totalServers, "server");

  const t21 = `${totalServers} ${t20}`;
  const t22 = SCOPE_ORDER.map(scope_0 => {
      const scopeServers_0 = serversByScope.get(scope_0);
      if (!scopeServers_0 || scopeServers_0.length === 0) {
        return null;
      }
      const heading = getScopeHeading(scope_0);
      return <Box key={scope_0} flexDirection="column" marginBottom={1}><Box paddingLeft={2}><Text bold={true}>{heading.label}</Text>{heading.path && <Text dimColor={true}> ({heading.path})</Text>}</Box>{scopeServers_0.map(server_4 => renderServerItem(server_4))}</Box>;
    });

  const t23 = claudeAiServers.length > 0 && <Box flexDirection="column" marginBottom={1}><Box paddingLeft={2}><Text bold={true}>Hosted connectors</Text></Box>{claudeAiServers.map(server_5 => renderServerItem(server_5))}</Box>;

  const t24 = agentServers.length > 0 && <Box flexDirection="column" marginBottom={1}><Box paddingLeft={2}><Text bold={true}>Agent MCPs</Text></Box>{[...new Set(agentServers.flatMap(_temp6))].map(agentName => <Box key={agentName} flexDirection="column" marginTop={1}><Box paddingLeft={2}><Text dimColor={true}>@{agentName}</Text></Box>{agentServers.filter(s_3 => s_3.sourceAgents.includes(agentName)).map(agentServer_2 => renderAgentServerItem(agentServer_2))}</Box>)}</Box>;

  const t25 = dynamicServers.length > 0 && <Box flexDirection="column" marginBottom={1}><Box paddingLeft={2}><Text bold={true}>{dynamicHeading.label}</Text>{dynamicHeading.path && <Text dimColor={true}> ({dynamicHeading.path})</Text>}</Box>{dynamicServers.map(server_6 => renderServerItem(server_6))}</Box>;

  const t26 = hasFailedClients && <Text dimColor={true}>{debugMode ? "\u203B Error logs shown inline with --debug" : "\u203B Run claude --debug to see error logs"}</Text>;

  const t27 = <Text dimColor={true}><Link url="https://docs.allternit.com/cli/mcp">https://docs.allternit.com/cli/mcp</Link>{" "}for help</Text>;

  const t28 = <Box flexDirection="column">{t26}{t27}</Box>;

  const t29 = <Box flexDirection="column">{t22}{t23}{t24}{t25}{t28}</Box>;

  const t30 = <Dialog title="Manage MCP servers" subtitle={t21} onCancel={handleCancel} hideInputGuide={true}>{t29}</Dialog>;

  const t31 = <Box paddingX={1}><Text dimColor={true} italic={true}><Byline><KeyboardShortcutHint shortcut={"\u2191\u2193"} action="navigate" /><KeyboardShortcutHint shortcut="Enter" action="confirm" /><ConfigurableShortcutHint action="confirm:no" context="Confirmation" fallback="Esc" description="cancel" /></Byline></Text></Box>;

  const t32 = <Box flexDirection="column">{t19}{t30}{t31}</Box>;

  return t32;
}
function _temp6(s_2) {
  return s_2.sourceAgents;
}
function _temp5(s_1) {
  return s_1.client.type === "failed";
}
function _temp4(a_0, b_0) {
  return a_0.name.localeCompare(b_0.name);
}
function _temp3(a, b) {
  return a.name.localeCompare(b.name);
}
function _temp2(s_0) {
  return s_0.client.config.type === "claudeai-proxy";
}
function _temp(s) {
  return s.client.config.type !== "claudeai-proxy";
}
