/** Standard MCP Apps (SEP-1865) method names and the minimal shapes a View touches. */
export const PROTOCOL_VERSION = "2026-01-26";

export const METHODS = {
  initialize: "ui/initialize",
  initialized: "ui/notifications/initialized",
  toolInput: "ui/notifications/tool-input",
  toolResult: "ui/notifications/tool-result",
  hostContextChanged: "ui/notifications/host-context-changed",
  sizeChanged: "ui/notifications/size-changed",
  teardown: "ui/resource-teardown",
  callTool: "tools/call",
  openLink: "ui/open-link",
  message: "ui/message",
  updateModelContext: "ui/update-model-context",
} as const;

export interface HostContext {
  theme?: "light" | "dark";
  locale?: string;
  displayMode?: "inline" | "fullscreen" | "pip";
  styles?: { variables?: Record<string, string | undefined> };
  containerDimensions?: { width?: number; height?: number; maxHeight?: number; maxWidth?: number };
  [key: string]: unknown;
}

export interface ToolResult {
  content?: Array<{ type: string; text?: string; [key: string]: unknown }>;
  structuredContent?: Record<string, unknown>;
  isError?: boolean;
  _meta?: Record<string, unknown>;
}

export interface ToolInput {
  arguments?: Record<string, unknown>;
}

export interface InitializeResult {
  protocolVersion?: string;
  hostContext?: HostContext;
  hostCapabilities?: Record<string, unknown>;
  hostInfo?: { name?: string; version?: string };
}
