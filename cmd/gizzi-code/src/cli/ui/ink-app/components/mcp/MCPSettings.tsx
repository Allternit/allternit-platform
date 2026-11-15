import React, { useEffect, useMemo } from 'react';
import type { CommandResultDisplay } from '../../commands';
import { ClaudeAuthProvider } from '../../services/mcp/auth';
import type { McpClaudeAIProxyServerConfig, McpHTTPServerConfig, McpSSEServerConfig, McpStdioServerConfig } from '../../services/mcp/types';
import { extractAgentMcpServers, filterToolsByServer } from '../../services/mcp/utils';
import { useAppState } from '../../state/AppState';
import { getSessionIngressAuthToken } from '../../utils/sessionIngressAuth';
import { MCPAgentServerMenu } from './MCPAgentServerMenu';
import { MCPListPanel } from './MCPListPanel';
import { MCPRemoteServerMenu } from './MCPRemoteServerMenu';
import { MCPStdioServerMenu } from './MCPStdioServerMenu';
import { MCPToolDetailView } from './MCPToolDetailView';
import { MCPToolListView } from './MCPToolListView';
import type { AgentMcpServerInfo, MCPViewState, ServerInfo } from './types';
type Props = {
  onComplete: (result?: string, options?: {
    display?: CommandResultDisplay;
  }) => void;
};
export function MCPSettings({
    onComplete
}: Props) {
  const mcp = useAppState(_temp);
  const agentDefinitions = useAppState(_temp2);
  const mcpClients = mcp.clients;
  const t1: MCPViewState = {
      type: "list"
    };

  const [viewState, setViewState] = React.useState<MCPViewState>(t1);
  const t2 = [];

  const [servers, setServers] = React.useState(t2);
  const t3 = extractAgentMcpServers(agentDefinitions.allAgents);

  const agentMcpServers = t3;
  const t4 = mcpClients.filter(_temp3).sort(_temp4);

  const filteredClients = t4;
  const t5 = () => {
      let cancelled = false;
      const prepareServers = async function prepareServers() {
        const serverInfos = await Promise.all(filteredClients.map(async client_0 => {
          const scope = client_0.config.scope;
          const isSSE = client_0.config.type === "sse";
          const isHTTP = client_0.config.type === "http";
          const isClaudeAIProxy = client_0.config.type === "claudeai-proxy";
          let isAuthenticated = undefined;
          if (isSSE || isHTTP) {
            const authProvider = new ClaudeAuthProvider(client_0.name, client_0.config as McpSSEServerConfig | McpHTTPServerConfig);
            const tokens = await authProvider.tokens();
            const hasSessionAuth = getSessionIngressAuthToken() !== null && client_0.type === "connected";
            const hasToolsAndConnected = client_0.type === "connected" && filterToolsByServer(mcp.tools, client_0.name).length > 0;
            isAuthenticated = Boolean(tokens) || hasSessionAuth || hasToolsAndConnected;
          }
          const baseInfo = {
            name: client_0.name,
            client: client_0,
            scope
          };
          if (isClaudeAIProxy) {
            return {
              ...baseInfo,
              transport: "claudeai-proxy" as const,
              isAuthenticated: false,
              config: client_0.config as McpClaudeAIProxyServerConfig
            };
          } else {
            if (isSSE) {
              return {
                ...baseInfo,
                transport: "sse" as const,
                isAuthenticated,
                config: client_0.config as McpSSEServerConfig
              };
            } else {
              if (isHTTP) {
                return {
                  ...baseInfo,
                  transport: "http" as const,
                  isAuthenticated,
                  config: client_0.config as McpHTTPServerConfig
                };
              } else {
                return {
                  ...baseInfo,
                  transport: "stdio" as const,
                  config: client_0.config as McpStdioServerConfig
                };
              }
            }
          }
        }));
        if (cancelled) {
          return;
        }
        setServers(serverInfos);
      };
      prepareServers();
      return () => {
        cancelled = true;
      };
    };
  const t6 = [filteredClients, mcp.tools];

  React.useEffect(t5, t6);
  const t7 = () => {
      if (servers.length === 0 && filteredClients.length > 0) {
        return;
      }
      if (servers.length === 0 && agentMcpServers.length === 0) {
        onComplete("No MCP servers configured. Please run /doctor if this is unexpected. Otherwise, run `claude mcp --help` or visit https://docs.allternit.com/cli/mcp to learn more.");
      }
    };
  const t8 = [servers.length, filteredClients.length, agentMcpServers.length, onComplete];

  useEffect(t7, t8);
  switch (viewState.type) {
    case "list":
      {
        const t9 = server => setViewState({
            type: "server-menu",
            server
          });
        const t10 = agentServer => setViewState({
            type: "agent-server-menu",
            agentServer
          });

        const t11 = <MCPListPanel servers={servers} agentServers={agentMcpServers} onSelectServer={t9} onSelectAgentServer={t10} onComplete={onComplete} defaultTab={viewState.defaultTab} />;

        return t11;
      }
    case "server-menu":
      {
        const t9 = filterToolsByServer(mcp.tools, viewState.server.name);

        const serverTools_0 = t9;
        const defaultTab = viewState.server.transport === "claudeai-proxy" ? "claude.ai" : "Gizzi Code";
        if (viewState.server.transport === "stdio") {
          const t10 = () => setViewState({
              type: "server-tools",
              server: viewState.server
            });

          const t11 = () => setViewState({
              type: "list",
              defaultTab
            });

          const t12 = <MCPStdioServerMenu server={viewState.server} serverToolsCount={serverTools_0.length} onViewTools={t10} onCancel={t11} onComplete={onComplete} />;

          return t12;
        } else {
          const t10 = () => setViewState({
              type: "server-tools",
              server: viewState.server
            });

          const t11 = () => setViewState({
              type: "list",
              defaultTab
            });

          const t12 = <MCPRemoteServerMenu server={viewState.server} serverToolsCount={serverTools_0.length} onViewTools={t10} onCancel={t11} onComplete={onComplete} />;

          return t12;
        }
      }
    case "server-tools":
      {
        const t9 = (_, index) => setViewState({
            type: "server-tool-detail",
            server: viewState.server,
            toolIndex: index
          });
        const t10 = () => setViewState({
            type: "server-menu",
            server: viewState.server
          });

        const t11 = <MCPToolListView server={viewState.server} onSelectTool={t9} onBack={t10} />;

        return t11;
      }
    case "server-tool-detail":
      {
        const t9 = filterToolsByServer(mcp.tools, viewState.server.name);

        const serverTools = t9;
        const tool = serverTools[viewState.toolIndex];
        if (!tool) {
          setViewState({
            type: "server-tools",
            server: viewState.server
          });
          return null;
        }
        const t10 = () => setViewState({
            type: "server-tools",
            server: viewState.server
          });

        const t11 = <MCPToolDetailView tool={tool} server={viewState.server} onBack={t10} />;

        return t11;
      }
    case "agent-server-menu":
      {
        const t9 = () => setViewState({
            type: "list",
            defaultTab: "Agents"
          });

        const t10 = <MCPAgentServerMenu agentServer={viewState.agentServer} onCancel={t9} onComplete={onComplete} />;

        return t10;
      }
  }
}
function _temp4(a, b) {
  return a.name.localeCompare(b.name);
}
function _temp3(client) {
  return client.name !== "ide";
}
function _temp2(s_0) {
  return s_0.agentDefinitions;
}
function _temp(s) {
  return s.mcp;
}
