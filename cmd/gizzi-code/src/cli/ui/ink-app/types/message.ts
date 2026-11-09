/**
 * Gizzi Message Type System
 * Complete implementation based on Claude's canonical architecture
 */

import type { BetaContentBlock, BetaToolUseBlock } from '@allternit/gizzi-sdk/providers/allternit/resources/beta/messages/messages.mjs'

type UUID = string

// ============================================================================
// Base Types
// ============================================================================

export type MessageRole = 'user' | 'assistant' | 'system'

export type SystemMessageLevel =
  | 'info'
  | 'warning'
  | 'error'
  | 'success'
  | 'suggestion'

type MessageUsage = {
  input_tokens: number
  output_tokens: number
}

type NestedMessage = {
  id?: string
  role?: MessageRole
  content: string | MessageContent[] | ContentBlock[] | unknown[] | unknown
  stop_reason?: string | null
  usage?: MessageUsage
  model?: string
  type?: string
  stop_sequence?: string
  container?: unknown
  context_management?: unknown
  [key: string]: any
}

/**
 * Type guard to check if content is an array of MessageContent
 */
export function isMessageContentArray(content: unknown): content is MessageContent[] {
  return Array.isArray(content) && content.length > 0 && typeof content[0] === 'object' && content[0] !== null && 'type' in content[0]
}

/**
 * Type guard to check if content is an array of ContentBlock
 */
export function isContentBlockArray(content: unknown): content is ContentBlock[] {
  return Array.isArray(content) && content.length > 0 && typeof content[0] === 'object' && content[0] !== null && 'type' in content[0]
}

/**
 * Get the first content item from a NestedMessage safely
 */
export function getFirstContentItem(content: string | MessageContent[] | ContentBlock[]): MessageContent | ContentBlock | undefined {
  if (typeof content === 'string') {
    return { type: 'text', text: content }
  }
  if (Array.isArray(content) && content.length > 0) {
    return content[0] as MessageContent | ContentBlock
  }
  return undefined
}

export interface MessageOrigin {
  type?: string
  kind?: string
  id?: string
  server?: string
}

export interface MessageAttachment {
  type?:
    | 'file'
    | 'image'
    | 'code'
    | 'companion_intro'
    | 'compact_file_reference'
    | 'pdf_reference'
    | 'already_read_file'
    | 'edited_text_file'
    | 'edited_image_file'
    | 'directory'
    | 'selected_lines_in_ide'
    | 'opened_file_in_ide'
    | 'todo_reminder'
    | 'task_reminder'
    | 'nested_memory'
    | 'relevant_memories'
    | 'dynamic_skill'
    | 'skill_listing'
    | 'skill_discovery'
    | 'queued_command'
    | 'output_style'
    | 'diagnostics'
    | 'plan_mode'
    | 'plan_mode_reentry'
    | 'plan_mode_exit'
    | 'auto_mode'
    | 'auto_mode_exit'
    | 'critical_system_reminder'
    | 'plan_file_reference'
    | 'mcp_resource'
    | 'command_permissions'
    | 'agent_mention'
    | 'task_status'
    | 'async_hook_response'
    | 'token_usage'
    | 'budget_usd'
    | 'output_token_usage'
    | 'structured_output'
    | 'invoked_skills'
    | 'max_turns_reached'
    | 'current_session_memory'
    | 'teammate_shutdown_batch'
    | 'compaction_reminder'
    | 'context_efficiency'
    | 'date_change'
    | 'ultrathink_effort'
    | 'deferred_tools_delta'
    | 'agent_listing_delta'
    | 'mcp_instructions_delta'
    | 'teammate_mailbox'
    | 'team_context'
    | 'bagel_console'
    | string
  name?: string
  fileName?: string
  content?: string | string[]
  mimeType?: string
  prompt?: string | Array<{type: string; text?: string}>
  identity?: {
    type: string
    name: string
  }
  // Properties for specific attachment types
  isMeta?: boolean
  commandMode?: string
  memories?: Array<{content: string}>
  addedNames?: string[]
  removedNames?: string[]
  // Hook-related properties
  hookEvent?: string
  hookName?: string
  toolUseID?: string
  tool_use_id?: string
  // Data payload for complex attachments
  data?: unknown
  parentToolUseID?: string
  parent_tool_use_id?: string
}

export interface ToolUseResult {
  toolUseId: string
  content: string | unknown[]
  isError?: boolean
}

export interface MessageContent {
  type: 'text' | 'tool_use' | 'tool_result' | 'thinking' | 'image' | 'redacted_thinking' | 'server_tool_use' | 'advisor_tool_result' | string
  text?: string
  id?: string
  name?: string
  input?: unknown
  content?: unknown
  thinking?: string
  signature?: string
  toolUseId?: string
  tool_use_id?: string  // Snake case alias
  is_error?: boolean
  source?: unknown
  // Additional properties for extended content types
  data?: string
  partial_json?: string
  index?: number
  caller?: unknown
  // Tool result content items
  toolUse?: {
    name: string
    input: Record<string, unknown>
  }
}

// Specific content block types for discriminated unions
export interface TextContentBlock {
  type: 'text'
  text: string
  [key: string]: unknown
}

export interface ToolUseContentBlock {
  type: 'tool_use'
  id: string
  name: string
  input: Record<string, unknown> | unknown
  // Additional properties for SDK compatibility
  toolUseId?: string
  tool_use_id?: string
  [key: string]: unknown
}

export interface ToolResultContentBlock {
  type: 'tool_result'
  tool_use_id: string
  content: string | Array<{type: string; text?: string; source?: unknown}>
  is_error?: boolean
  [key: string]: unknown
}

/**
 * Tool reference content block (beta feature).
 * Used for dynamic tool loading - contains a reference to a tool by name
 * that gets expanded into the full tool definition in the API.
 */
export interface ToolReferenceContentBlock {
  type: 'tool_reference'
  tool_name: string
}

export interface ThinkingContentBlock {
  type: 'thinking'
  thinking: string
  signature?: string
  [key: string]: unknown
}

export interface RedactedThinkingContentBlock {
  type: 'redacted_thinking'
  data: string
  [key: string]: unknown
}

export interface ImageContentBlock {
  type: 'image'
  source: {
    type: 'base64' | 'url'
    media_type: string
    data: string
  }
  [key: string]: unknown
}

export type ContentBlock = TextContentBlock | ToolUseContentBlock | ToolResultContentBlock | ThinkingContentBlock | RedactedThinkingContentBlock | ImageContentBlock

export interface CompactMetadata {
  messageCount?: number
  direction?: 'up' | 'down'
  /** Tool names discovered before compaction, preserved across compact boundaries */
  preCompactDiscoveredTools?: string[]
  [key: string]: unknown
}

export interface McpMeta {
  serverName?: string
  _meta?: Record<string, unknown>
  structuredContent?: Record<string, unknown>
  [key: string]: unknown
}

export interface ToolUseSummary {
  toolUseId: string
  name: string
  status: 'pending' | 'running' | 'completed' | 'error'
  result?: string
}

// ============================================================================
// Base Message Interface
// ============================================================================

export interface Message {
  // Core properties
  type: MessageRole | 'progress' | 'system' | 'tombstone' | 'attachment' | 'grouped_tool_use' | 'collapsed_read_search' | 'stream_event' | 'stream_request_start' | 'tool_use_summary'
  role?: MessageRole
  uuid: UUID | string
  id?: string  // Alternative ID property
  content?: string | MessageContent[] | ContentBlock[] | unknown[]
  
  // Stream event properties
  event?: {
    type: 'message_start' | 'message_delta' | 'message_stop' | 'content_block_start' | 'content_block_delta' | 'content_block_stop'
    message?: {
      usage?: MessageUsage
    }
    delta?: {
      type?: string
      text?: string
      partial_json?: string
      thinking?: string
      stop_reason?: string | null
    }
    content_block?: BetaContentBlock | ContentBlock | MessageContent
    index?: number
    usage?: MessageUsage
  }
  
  // Message state
  stop_reason?: string | null
  model?: string
  usage?: MessageUsage
  
  // Session context
  session_id?: string
  parent_tool_use_id?: string | null
  parentToolUseID?: string | null
  isCompactSummary?: boolean
  
  // Nested message (for compact summaries)
  message?: NestedMessage
  
  // UI/Display properties
  isVirtual?: boolean
  isMeta?: boolean
  isVisibleInTranscriptOnly?: boolean
  isApiErrorMessage?: boolean
  subtype?: 
    | 'user' 
    | 'assistant' 
    | 'system' 
    | 'attachment'
    | 'compact_boundary'
    | 'tool_summary'
    | 'thinking'
    | 'memory_saved'
    | 'memory_updated'
    | 'stop_hook'
    | 'api_error'
    | 'informational'
    | 'bridge_status'
    | 'local_command'
    | 'api_metrics'
    | 'stop_hook_summary'
    | 'turn_duration'
    | 'run_telemetry'
    | 'microcompact_boundary'
    | 'agents_killed'
    | 'away_summary'
    | 'hook_result'
    | 'hook_progress'
    | 'hook_response'
    | 'hook_started'
    | 'success'
    | 'file_snapshot'
    | 'permission_retry'
    | 'scheduled_task_fire'
    | string
  
  // Origin and context
  origin?: MessageOrigin
  
  // Tool and attachment data
  toolUseResult?: ToolUseResult
  toolUseID?: string
  toolUseSummary?: ToolUseSummary
  attachment?: MessageAttachment
  client?: unknown
  source?: unknown
  data?: {
    toolUseId?: string
    content?: unknown
    type?: string
    [key: string]: unknown
  }
  
  // Metadata
  timestamp?: string | number
  requestId?: string
  error?: Error | string
  apiError?: unknown
  errorDetails?: string
  advisorModel?: string
  mcpMeta?: McpMeta
  imagePasteIds?: Array<string | number>
  compactMetadata?: CompactMetadata
  messageCount?: number
  permissionMode?: string
  level?: SystemMessageLevel
  preventContinuation?: boolean
  commands?: string[]
  url?: string
  upgradeNudge?: string
  hookCount?: number
  hookInfos?: StopHookInfo[]
  hookErrors?: string[]
  stopReason?: string
  hasOutput?: boolean
  hookLabel?: string
  totalDurationMs?: number
  durationMs?: number
  budgetTokens?: number
  budgetLimit?: number
  budgetNudges?: number
  writtenPaths?: string[]
  microcompactMetadata?: {
    trigger: 'auto'
    preTokens: number
    tokensSaved: number
    compactedToolIds: string[]
    clearedAttachmentUUIDs: string[]
  }
  ttftMs?: number
  otps?: number
  isP50?: boolean
  hookDurationMs?: number
  turnDurationMs?: number
  toolDurationMs?: number
  classifierDurationMs?: number
  toolCount?: number
  classifierCount?: number
  configWriteCount?: number
  cause?: Error
  retryInMs?: number
  retryAttempt?: number
  maxRetries?: number
  
  // Agent/workspace
  agentDefinitions?: unknown[]
  
  // Additional properties for various message types
  text?: string
  name?: string
  command?: string
  tool_use_id?: string
  output?: string
  stdout?: string
  result?: unknown
  status?: 'connected' | 'disconnected' | 'reconnecting'
  taskName?: string
  scheduledTime?: number
  
  // For collapsed read search groups
  messages?: Message[]
  summary?: string
  
  // For progress messages
  toolUseId?: string
  
  // For permission/system messages  
  toolName?: string
  
  // Array-like properties (for filter, length, etc.)
  length?: number
  filter?: unknown
  
  // Catch-all for any other properties used in the codebase
  [key: string]: any
}

// ============================================================================
// Role-Specific Message Types
// ============================================================================

export interface UserMessage extends Message {
  type: 'user'
  role?: 'user'
  message: NestedMessage & {
    role: 'user'
    content: string | MessageContent[] | ContentBlock[]
  }
}

export interface AssistantMessage extends Message {
  type: 'assistant'
  role?: 'assistant'
  message: NestedMessage & {
    role: 'assistant'
    content: string | MessageContent[] | ContentBlock[]
  }
  /** Tool calls a CLI brain already ran itself: shown, never run again (services/api/cliBrain.ts). */
  cliObserved?: true
}

export interface SystemMessage extends Message {
  type: 'system'
  role?: 'system'
  content?: string
  level?: SystemMessageLevel
}

// ============================================================================
// UI/Display Message Types
// ============================================================================

/**
 * RenderableMessage - Messages that can be rendered in the UI
 * Includes display properties for the TUI
 */
export interface RenderableMessage extends Message {
  isRenderable: true
  renderOptions?: {
    collapsible?: boolean
    highlight?: boolean
  }
  messages?: Message[]
  // Properties for collapsed_read_search type
  relevantMemories?: Array<{content: string}>
}

/**
 * AttachmentMessage - Messages containing file attachments
 */
export interface AttachmentMessage<T = MessageAttachment> extends Message {
  type: 'attachment'
  subtype: 'attachment'
  attachment: T
  fileName?: string
  mimeType?: string
}

// ============================================================================
// System Message Subtypes
// ============================================================================

/**
 * SystemCompactBoundaryMessage - Marks snip boundaries in conversation
 */
export interface SystemCompactBoundaryMessage extends SystemMessage {
  subtype: 'compact_boundary'
  compactMetadata: CompactMetadata
  messageCount?: number
  direction?: 'up' | 'down'
}

/**
 * SystemAPIErrorMessage - API error messages
 */
export interface SystemAPIErrorMessage extends SystemMessage {
  subtype: 'api_error'
  error: string | Error
  cause?: Error
  retryInMs?: number
  retryAttempt?: number
  maxRetries?: number
  isApiErrorMessage?: true
}

/**
 * SystemMemorySavedMessage - Memory persistence notifications
 */
export interface SystemMemorySavedMessage extends SystemMessage {
  subtype: 'memory_saved'
}

/**
 * SystemMemoryUpdatedMessage - `#` quick-add confirmation banner
 * ("Memory updated in X · /memory to edit").
 */
export interface SystemMemoryUpdatedMessage extends SystemMessage {
  subtype: 'memory_updated'
  memoryPath: string
}

/**
 * SystemStopHookSummaryMessage - Hook execution summaries
 */
export interface SystemStopHookSummaryMessage extends SystemMessage {
  subtype: 'stop_hook_summary'
  hookCount: number
  hookInfos: StopHookInfo[]
  hookErrors: string[]
  preventedContinuation: boolean
  stopReason?: string
  hasOutput: boolean
  hookLabel?: string
  totalDurationMs?: number
}

/**
 * SystemThinkingMessage - Assistant thinking/reasoning display
 */
export interface SystemThinkingMessage extends SystemMessage {
  subtype: 'thinking'
  thinking: string
  signature?: string
}

/**
 * SystemTurnDurationMessage - Turn timing information
 */
export interface SystemTurnDurationMessage extends SystemMessage {
  subtype: 'turn_duration'
  durationMs: number
  budgetTokens?: number
  budgetLimit?: number
  budgetNudges?: number
  messageCount?: number
}

/**
 * SystemRunTelemetryMessage - Per-turn telemetry line (model, wall time,
 * tokens, cost, context bar, plan-quota chip). Every field optional —
 * absent data means the segment was omitted, never zero-filled.
 */
export interface SystemRunTelemetryMessage extends SystemMessage {
  subtype: 'run_telemetry'
  content: string
  modelDisplay?: string
  durationMs?: number
  inputTokens?: number
  outputTokens?: number
  usageEstimated?: boolean
  toolCount?: number
  costUSD?: number
  contextRatio?: number
  contextEstimated?: boolean
  quotaChip?: string
  /** Files the turn changed, for "Done in … · 3 files +48 −12" and the list under it. */
  files?: { path: string; added: number; removed: number }[]
  /** Set when the turn failed or produced nothing: no "Done" line, "✕ Stopped …" instead. */
  stopped?: { cause?: string }
}

/**
 * SystemBridgeStatusMessage - Bridge connection status
 */
export interface SystemBridgeStatusMessage extends SystemMessage {
  subtype: 'bridge_status'
  url: string
  upgradeNudge?: string
}

/**
 * SystemInformationalMessage - General system information
 */
export interface SystemInformationalMessage extends SystemMessage {
  subtype: 'informational'
  content: string
}

/**
 * SystemAgentsKilledMessage - Notification when agents are killed
 */
export interface SystemAgentsKilledMessage extends SystemMessage {
  subtype: 'agents_killed'
}

/**
 * SystemAwaySummaryMessage - Summary generated while away
 */
export interface SystemAwaySummaryMessage extends SystemMessage {
  subtype: 'away_summary'
  content: string
}

/**
 * SystemLocalCommandMessage - Local command execution message
 */
export interface SystemLocalCommandMessage extends SystemMessage {
  subtype: 'local_command'
  content: string
}

export interface SystemApiMetricsMessage extends SystemMessage {
  subtype: 'api_metrics'
  ttftMs: number
  otps: number
  isP50?: boolean
  hookDurationMs?: number
  turnDurationMs?: number
  toolDurationMs?: number
  classifierDurationMs?: number
  toolCount?: number
  hookCount?: number
  classifierCount?: number
  configWriteCount?: number
}

export interface SystemMicrocompactBoundaryMessage extends SystemMessage {
  subtype: 'microcompact_boundary'
  content: string
  microcompactMetadata: {
    trigger: 'auto'
    preTokens: number
    tokensSaved: number
    compactedToolIds: string[]
    clearedAttachmentUUIDs: string[]
  }
}

/**
 * SystemPermissionRetryMessage - Permission retry request
 */
export interface SystemPermissionRetryMessage extends SystemMessage {
  subtype: 'permission_retry'
  commands: string[]
}

/**
 * SystemScheduledTaskFireMessage - Scheduled task execution notification
 */
export interface SystemScheduledTaskFireMessage extends SystemMessage {
  subtype: 'scheduled_task_fire'
  content: string
}

// ============================================================================
// Tool-Related Message Types
// ============================================================================

/**
 * ToolUseSummaryMessage - Summary of tool execution
 */
export interface ToolUseSummaryMessage extends Message {
  type: 'tool_use_summary'
  subtype?: 'tool_summary'
  toolUseSummary?: ToolUseSummary
  summary: string
  precedingToolUseIds: string[]
}

/**
 * GroupedToolUseMessage - Grouped tool use results
 *
 * Contract shared by the producer (utils/groupToolUses.ts applyGrouping) and
 * the renderer (components/messages/GroupedToolUseContent.tsx): the grouped
 * assistant tool_use carriers, their result user messages, and the API
 * message id the group was keyed on.
 */
export interface GroupedToolUseMessage extends Message {
  type: 'grouped_tool_use'
  toolName: string
  messages: NormalizedAssistantMessage<BetaToolUseBlock>[]
  results: UserMessage[]
  displayMessage: NormalizedAssistantMessage<BetaToolUseBlock>
  messageId: string
}

/**
 * HookResultMessage - Hook execution results
 */
export interface HookResultMessage extends Message {
  subtype: 'hook_result'
  hookName: string
  result: unknown
}

// ============================================================================
// Special Message Types
// ============================================================================

/**
 * ProgressMessage - Streaming progress updates
 */
export interface ProgressMessage<T = unknown> {
  type: 'progress'
  toolUseID?: string
  parentToolUseID?: string
  uuid?: string
  timestamp?: string
  data?: {
    type?: string
    hookEvent?: string
  } & T
  [key: string]: unknown
  // Progress messages may contain nested message data for streaming
  message?: {
    content: MessageContent[] | ContentBlock[]
    role?: MessageRole
    id?: string
    type?: string
  }
}

/**
 * CollapsedReadSearchGroup - Collapsed search results
 */
export interface CollapsedReadSearchGroup {
  type: 'collapsed_read_search'
  messages: Message[]
  summary: string
  toolName?: string
  displayMessage: Message
  uuid: UUID | string
  timestamp?: string | number
  // Count properties for collapsed search/read operations
  searchCount?: number
  readCount?: number
  listCount?: number
  replCount?: number
  memorySearchCount?: number
  memoryReadCount?: number
  memoryWriteCount?: number
  teamMemorySearchCount?: number
  teamMemoryReadCount?: number
  teamMemoryWriteCount?: number
  readFilePaths?: string[]
  searchArgs?: string[]
  latestDisplayHint?: string
  mcpCallCount?: number
  mcpServerNames?: string[]
  bashCount?: number
  gitOpBashCount?: number
  commits?: Array<{sha: string; kind: string}>
  pushes?: Array<{branch: string}>
  branches?: Array<{ref: string; action: string}>
  prs?: Array<{number: number; url?: string; action?: string}>
  hookTotalMs?: number
  hookCount?: number
  hookInfos?: StopHookInfo[]
  relevantMemories?: Array<{path: string; content: string; mtimeMs: number}>
}

/**
 * TombstoneMessage - Deleted message placeholder
 */
export interface TombstoneMessage extends Message {
  type: 'tombstone'
  originalUuid: string
  deletedAt: number
}

/**
 * QueueOperationMessage - Async operation queue
 */
export interface QueueOperationMessage extends Message {
  operation: 'enqueue' | 'dequeue'
  queueName: string
}

// ============================================================================
// Stream Event Types
// ============================================================================

/**
 * StreamEvent - Events emitted during message streaming
 */
export interface StreamEvent {
  type: 'content' | 'error' | 'done' | 'tool_use' | 'tool_result'
  content?: string
  delta?: string
  toolUse?: unknown
  error?: Error
}

/**
 * RequestStartEvent - Emitted when a request starts
 */
export interface RequestStartEvent {
  type: 'request_start'
  requestId: string
  timestamp: number
}

// ============================================================================
// Compact Direction Type
// ============================================================================

/**
 * PartialCompactDirection - Direction for partial compaction
 */
export type PartialCompactDirection = 'up' | 'down' | 'both' | 'from' | 'up_to'

// ============================================================================
// Stop Hook Info
// ============================================================================

/**
 * StopHookInfo - Information about a stop hook execution
 */
export interface StopHookInfo {
  name: string
  duration: number
  result?: unknown
  command?: string
  durationMs?: number
  [key: string]: unknown
}

// ============================================================================
// Normalized Types (for internal processing)
// ============================================================================

export type NormalizedMessage = UserMessage | AssistantMessage | SystemMessage

export interface NormalizedUserMessage extends UserMessage {
  normalized: true
  originalContent?: string
}

// Normalized assistant message for SDK processing
export interface NormalizedAssistantMessage<
  T extends MessageContent | ContentBlock = MessageContent | ContentBlock,
> extends AssistantMessage {
  normalized: true
  content: string | MessageContent[]
  role: 'assistant'
  message: NestedMessage & {
    role: 'assistant'
    content: [T]
  }
}

// ============================================================================
// SDK Types
// ============================================================================

/**
 * SDKPartialAssistantMessage - Streaming assistant message chunks
 */
export interface SDKPartialAssistantMessage {
  type: 'assistant' | 'stream_event'
  event: 'content_block_delta' | 'content_block_start' | 'content_block_stop' | 'message_delta' | 'message_start' | 'message_stop'
  uuid: string
  session_id: string
  parent_tool_use_id: string | null
  [key: string]: unknown
  content?: string
  delta?: {
    type: string
    text?: string
    partial_json?: string
  }
  index?: number
  message?: {
    id?: string
    role: 'assistant'
    content: MessageContent[]
    stop_reason?: string | null
    model?: string
    usage?: {
      input_tokens: number
      output_tokens: number
    }
  }
  usage?: {
    input_tokens: number
    output_tokens: number
  }
  stop_reason?: string | null
}

// ============================================================================
// Utility Types
// ============================================================================

export type AnyMessage = 
  | Message 
  | UserMessage 
  | AssistantMessage 
  | SystemMessage
  | RenderableMessage
  | AttachmentMessage
  | SystemCompactBoundaryMessage
  | SystemAPIErrorMessage
  | ToolUseSummaryMessage
  | ProgressMessage
  | TombstoneMessage

export type MessageSubtype = Extract<Message['subtype'], string>

// Type guards
export function isAttachmentMessage(msg: Message): msg is AttachmentMessage {
  return msg.subtype === 'attachment'
}

export function isSystemCompactBoundaryMessage(msg: Message): msg is SystemCompactBoundaryMessage {
  return msg.subtype === 'compact_boundary'
}

export function isSystemAPIErrorMessage(msg: Message): msg is SystemAPIErrorMessage {
  return msg.isApiErrorMessage === true || msg.subtype === 'api_error'
}

export function isToolUseSummaryMessage(msg: Message): msg is ToolUseSummaryMessage {
  return msg.subtype === 'tool_summary'
}

// Content type guards
export function isTextContentBlock(content: MessageContent): content is TextContentBlock {
  return content.type === 'text'
}

export function isToolUseContentBlock(content: MessageContent): content is ToolUseContentBlock {
  return content.type === 'tool_use'
}

export function isToolResultContentBlock(content: MessageContent): content is ToolResultContentBlock {
  return content.type === 'tool_result'
}

// Helper type for nested message with array content
export type NestedMessageWithArrayContent = {
  id?: string
  role?: MessageRole
  type?: string
  content: MessageContent[] | ContentBlock[]
  stop_reason?: string | null
  usage?: {
    input_tokens: number
    output_tokens: number
  }
}

// Type guard for nested message with array content
export function hasArrayContent(message: { content?: string | MessageContent[] | ContentBlock[] }): message is { content: MessageContent[] | ContentBlock[] } {
  return Array.isArray(message.content)
}

// Helper to get the first content block from a message's content
export function getFirstContentBlock(message: { content?: string | MessageContent[] | ContentBlock[] }): MessageContent | ContentBlock | undefined {
  if (Array.isArray(message.content) && message.content.length > 0) {
    return message.content[0] as MessageContent | ContentBlock
  }
  return undefined
}

// Type guard for tool use content blocks
export function isToolUseBlock(content: MessageContent | ContentBlock): content is ToolUseContentBlock {
  return content.type === 'tool_use'
}

// Type guard for text content blocks
export function isTextBlock(content: MessageContent | ContentBlock): content is TextContentBlock {
  return content.type === 'text'
}

// Type guard for tool result content blocks
export function isToolResultBlock(content: MessageContent | ContentBlock): content is ToolResultContentBlock {
  return content.type === 'tool_result'
}

// Type guard for thinking content blocks
export function isThinkingBlock(content: MessageContent | ContentBlock): content is ThinkingContentBlock {
  return content.type === 'thinking'
}

// Type guard for redacted thinking content blocks
export function isRedactedThinkingBlock(content: MessageContent | ContentBlock): content is RedactedThinkingContentBlock {
  return content.type === 'redacted_thinking'
}

// ============================================================================
// Helper Types for Array Content
// ============================================================================

// Message where nested message content is guaranteed to be an array
export interface MessageWithArrayContent extends Message {
  message: {
    id?: string
    role?: MessageRole
    content: MessageContent[] | ContentBlock[]
    stop_reason?: string | null
    usage?: {
      input_tokens: number
      output_tokens: number
    }
  }
}

// ============================================================================
// Collapsible Message Type
// ============================================================================

export interface CollapsibleMessage extends Message {
  collapsible: true
  collapsed: boolean
  summary?: string
  children?: Message[]
}

// ============================================================================
// File Snapshot Message Type
// ============================================================================

export interface SystemFileSnapshotMessage extends SystemMessage {
  subtype: 'file_snapshot'
  snapshotFiles: Array<{ key: string; path: string; content: string }>
  timestamp: number
}
