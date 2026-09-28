import { feature } from 'bun:bundle';
import type { BetaContentBlock } from '@allternit/gizzi-sdk/providers/allternit/resources/beta/messages/messages.mjs';
import type { ImageBlockParam, TextBlockParam, ThinkingBlockParam, ToolResultBlockParam, ToolUseBlockParam } from '@allternit/gizzi-sdk/providers/allternit/resources/index.mjs';
import * as React from 'react';
import type { Command } from '../commands';
import { useTerminalSize } from '../hooks/useTerminalSize';
import { Box } from '../ink';
import type { Tools } from '../Tool';
import { type ConnectorTextBlock, isConnectorTextBlock } from '../types/connectorText';
import type { Attachment } from '../utils/attachments';
import type { AssistantMessage, AttachmentMessage as AttachmentMessageType, CollapsedReadSearchGroup as CollapsedReadSearchGroupType, ContentBlock, GroupedToolUseMessage as GroupedToolUseMessageType, MessageContent, NormalizedUserMessage, ProgressMessage, SystemMessage } from '../types/message';
import { type AdvisorBlock, isAdvisorBlock } from '../utils/advisor';
import { logError } from '../utils/log';
import type { buildMessageLookups } from '../utils/messages';
import { CompactSummary } from './CompactSummary';
import { AdvisorMessage } from './messages/AdvisorMessage';
import { AssistantRedactedThinkingMessage } from './messages/AssistantRedactedThinkingMessage';
import { AssistantTextMessage } from './messages/AssistantTextMessage';
import { AssistantThinkingMessage } from './messages/AssistantThinkingMessage';
import { AssistantToolUseMessage } from './messages/AssistantToolUseMessage';
import { AttachmentMessage } from './messages/AttachmentMessage';
import { CollapsedReadSearchContent } from './messages/CollapsedReadSearchContent';
import { CompactBoundaryMessage } from './messages/CompactBoundaryMessage';
import { GroupedToolUseContent } from './messages/GroupedToolUseContent';
import { SystemTextMessage } from './messages/SystemTextMessage';
import { UserImageMessage } from './messages/UserImageMessage';
import { UserTextMessage } from './messages/UserTextMessage';
import { UserToolResultMessage } from './messages/UserToolResultMessage/UserToolResultMessage';
import { OffscreenFreeze } from './OffscreenFreeze';
import { ExpandShellOutputProvider } from './shell/ExpandShellOutputContext';
export type Props = {
  message: NormalizedUserMessage | AssistantMessage | AttachmentMessageType | SystemMessage | GroupedToolUseMessageType | CollapsedReadSearchGroupType;
  lookups: ReturnType<typeof buildMessageLookups>;
  // TODO: Find a way to remove this, and leave spacing to the consumer
  /** Absolute width for the container Box. When provided, eliminates a wrapper Box in the caller. */
  containerWidth?: number;
  addMargin: boolean;
  tools: Tools;
  commands: Command[];
  verbose: boolean;
  inProgressToolUseIDs: Set<string>;
  progressMessagesForMessage: ProgressMessage[];
  shouldAnimate: boolean;
  shouldShowDot: boolean;
  style?: 'condensed';
  width?: number | string;
  isTranscriptMode: boolean;
  isStatic: boolean;
  onOpenRateLimitOptions?: () => void;
  isActiveCollapsedGroup?: boolean;
  isUserContinuation?: boolean;
  /** ID of the last thinking block (uuid:index) to show, used for hiding past thinking in transcript mode */
  lastThinkingBlockId?: string | null;
  /** UUID of the latest user bash output message (for auto-expanding) */
  latestBashOutputUUID?: string | null;
};
function MessageImpl({
    message,
    lookups,
    containerWidth,
    addMargin,
    tools,
    commands,
    verbose,
    inProgressToolUseIDs,
    progressMessagesForMessage,
    shouldAnimate,
    shouldShowDot,
    style,
    width,
    isTranscriptMode,
    onOpenRateLimitOptions,
    isActiveCollapsedGroup,
    isUserContinuation: t1,
    lastThinkingBlockId,
    latestBashOutputUUID
}: Props) {
  const isUserContinuation = t1 === undefined ? false : t1;
  switch (message.type) {
    case "attachment":
      {
        const t2 = <AttachmentMessage addMargin={addMargin} attachment={message.attachment as unknown as Attachment} verbose={verbose} isTranscriptMode={isTranscriptMode} />;

        return t2;
      }
    case "assistant":
      {
        const t2 = containerWidth ?? "100%";
        const t4 = (_, index_0) => <AssistantMessageBlock key={index_0} param={_} addMargin={addMargin} tools={tools} commands={commands} verbose={verbose} inProgressToolUseIDs={inProgressToolUseIDs} progressMessagesForMessage={progressMessagesForMessage} shouldAnimate={shouldAnimate} shouldShowDot={shouldShowDot} width={width} inProgressToolCallCount={inProgressToolUseIDs.size} isTranscriptMode={isTranscriptMode} lookups={lookups} onOpenRateLimitOptions={onOpenRateLimitOptions} thinkingBlockId={`${message.uuid}:${index_0}`} lastThinkingBlockId={lastThinkingBlockId} advisorModel={message.advisorModel} />;

        const t3 = (message.message.content as Array<MessageContent | ContentBlock>).map(t4);

        const t4_2 = <Box flexDirection="column" width={t2}>{t3}</Box>;

        return t4_2;
      }
    case "user":
      {
        if (message.isCompactSummary) {
          const t2 = isTranscriptMode ? "transcript" : "prompt";
          const t3 = <CompactSummary message={message} screen={t2} />;

          return t3;
        }
        const imageIndices = [];
        let imagePosition = 0;
        for (const param of message.message.content as Array<MessageContent | ContentBlock>) {
            if (param.type === "image") {
              const id = message.imagePasteIds?.[imagePosition];
              imagePosition++;
              imageIndices.push(id ?? imagePosition);
            } else {
              imageIndices.push(imagePosition);
            }
          }

        const isLatestBashOutput = latestBashOutputUUID === message.uuid;
        const t2 = containerWidth ?? "100%";
        const t3 = (message.message.content as Array<MessageContent | ContentBlock>).map((param_0, index) => <UserMessage key={index} message={message} addMargin={addMargin} tools={tools} progressMessagesForMessage={progressMessagesForMessage} param={param_0} style={style} verbose={verbose} imageIndex={imageIndices[index]} isUserContinuation={isUserContinuation} lookups={lookups} isTranscriptMode={isTranscriptMode} />);

        const t4 = <Box flexDirection="column" width={t2}>{t3}</Box>;

        const content = t4;
        const t5 = isLatestBashOutput ? <ExpandShellOutputProvider>{content}</ExpandShellOutputProvider> : content;

        return t5;
      }
    case "system":
      {
        if (message.subtype === "compact_boundary") {
          // Drawn in fullscreen too: the earlier conversation stays in
          // scrollback above the rip there.
          const t2 = <CompactBoundaryMessage metadata={message.compactMetadata} timestamp={typeof message.timestamp === 'number' ? new Date(message.timestamp).toISOString() : message.timestamp} />;

          return t2;
        }
        if (message.subtype === "microcompact_boundary") {
          return null;
        }
        if (feature("HISTORY_SNIP")) {
          // Structural cast: snipProjection is a nocheck shim whose broken
          // braces defeat typeof-import binding; the require shape is what
          // the runtime actually consumes.
          const {
            isSnipBoundaryMessage
          } = require("../services/compact/snipProjection.js") as {
            isSnipBoundaryMessage: (message: unknown) => boolean;
          };
          const {
            isSnipMarkerMessage
          } = require("../services/compact/snipCompact.js") as {
            isSnipMarkerMessage: (message: unknown) => boolean;
          };
          if (isSnipBoundaryMessage(message)) {
            const t2 = require("./messages/SnipBoundaryMessage.js");

            const {
              SnipBoundaryMessage
            } = t2 as {
              SnipBoundaryMessage: React.ComponentType<{
                message: unknown;
              }>;
            };
            const t3 = <SnipBoundaryMessage message={message} />;

            return t3;
          }
          if (isSnipMarkerMessage(message)) {
            return null;
          }
        }
        if (message.subtype === "local_command") {
          const t2 = {
              type: "text" as const,
              text: message.content
            };

          const t3 = <UserTextMessage addMargin={addMargin} param={t2} verbose={verbose} isTranscriptMode={isTranscriptMode} />;

          return t3;
        }
        const t2 = <SystemTextMessage message={message} addMargin={addMargin} verbose={verbose} isTranscriptMode={isTranscriptMode} />;

        return t2;
      }
    case "grouped_tool_use":
      {
        const t2 = <GroupedToolUseContent message={message} tools={tools} lookups={lookups} inProgressToolUseIDs={inProgressToolUseIDs} shouldAnimate={shouldAnimate} />;

        return t2;
      }
    case "collapsed_read_search":
      {
        const t2 = verbose || isTranscriptMode;
        const t3 = <OffscreenFreeze><CollapsedReadSearchContent message={message} inProgressToolUseIDs={inProgressToolUseIDs} shouldAnimate={shouldAnimate} verbose={t2} tools={tools} lookups={lookups} isActiveGroup={isActiveCollapsedGroup} /></OffscreenFreeze>;

        return t3;
      }
  }
}
function UserMessage(t0) {
  const {
    message,
    addMargin,
    tools,
    progressMessagesForMessage,
    param,
    style,
    verbose,
    imageIndex,
    isUserContinuation,
    lookups,
    isTranscriptMode
  } = t0;
  const {
    columns
  } = useTerminalSize();
  switch (param.type) {
    case "text":
      {
        const t1 = <UserTextMessage addMargin={addMargin} param={param} verbose={verbose} planContent={message.planContent} isTranscriptMode={isTranscriptMode} timestamp={message.timestamp} />;

        return t1;
      }
    case "image":
      {
        const t1 = addMargin && !isUserContinuation;
        const t2 = <UserImageMessage imageId={imageIndex} addMargin={t1} />;

        return t2;
      }
    case "tool_result":
      {
        const t1 = columns - 5;
        const t2 = <UserToolResultMessage param={param} message={message} lookups={lookups} progressMessagesForMessage={progressMessagesForMessage} style={style} tools={tools} verbose={verbose} width={t1} isTranscriptMode={isTranscriptMode} />;

        return t2;
      }
    default:
      {
        return;
      }
  }
}
function AssistantMessageBlock(t0) {
  const {
    param,
    addMargin,
    tools,
    commands,
    verbose,
    inProgressToolUseIDs,
    progressMessagesForMessage,
    shouldAnimate,
    shouldShowDot,
    width,
    inProgressToolCallCount,
    isTranscriptMode,
    lookups,
    onOpenRateLimitOptions,
    thinkingBlockId,
    lastThinkingBlockId,
    advisorModel
  } = t0;
  if (feature("CONNECTOR_TEXT")) {
    if (isConnectorTextBlock(param)) {
      const t1 = {
          type: "text" as const,
          text: param.connector_text
        };

      const t2 = <AssistantTextMessage param={t1} addMargin={addMargin} shouldShowDot={shouldShowDot} verbose={verbose} width={width} onOpenRateLimitOptions={onOpenRateLimitOptions} />;

      return t2;
    }
  }
  switch (param.type) {
    case "tool_use":
      {
        const t1 = <AssistantToolUseMessage param={param} addMargin={addMargin} tools={tools} commands={commands} verbose={verbose} inProgressToolUseIDs={inProgressToolUseIDs} progressMessagesForMessage={progressMessagesForMessage} shouldAnimate={shouldAnimate} shouldShowDot={shouldShowDot} inProgressToolCallCount={inProgressToolCallCount} lookups={lookups} isTranscriptMode={isTranscriptMode} />;

        return t1;
      }
    case "text":
      {
        const t1 = <AssistantTextMessage param={param} addMargin={addMargin} shouldShowDot={shouldShowDot} verbose={verbose} width={width} onOpenRateLimitOptions={onOpenRateLimitOptions} />;

        return t1;
      }
    case "redacted_thinking":
      {
        if (!isTranscriptMode && !verbose) {
          return null;
        }
        const t1 = <AssistantRedactedThinkingMessage addMargin={addMargin} />;

        return t1;
      }
    case "thinking":
      {
        if (!isTranscriptMode && !verbose) {
          return null;
        }
        const isLastThinking = !lastThinkingBlockId || thinkingBlockId === lastThinkingBlockId;
        const t1 = isTranscriptMode && !isLastThinking;
        const t2 = <AssistantThinkingMessage addMargin={addMargin} param={param} isTranscriptMode={isTranscriptMode} verbose={verbose} hideInTranscript={t1} />;

        return t2;
      }
    case "server_tool_use":
    case "advisor_tool_result":
      {
        if (isAdvisorBlock(param)) {
          const t1 = verbose || isTranscriptMode;
          const t2 = <AdvisorMessage block={param} addMargin={addMargin} resolvedToolUseIDs={lookups.resolvedToolUseIDs} erroredToolUseIDs={lookups.erroredToolUseIDs} shouldAnimate={shouldAnimate} verbose={t1} advisorModel={advisorModel} />;

          return t2;
        }
        logError(new Error(`Unable to render server tool block: ${param.type}`));
        return null;
      }
    default:
      {
        logError(new Error(`Unable to render message type: ${param.type}`));
        return null;
      }
  }
}
export function hasThinkingContent(m: {
  type: string;
  message?: {
    content: Array<{
      type: string;
    }>;
  };
}): boolean {
  if (m.type !== 'assistant' || !m.message) return false;
  return m.message.content.some(b => b.type === 'thinking' || b.type === 'redacted_thinking');
}

/** Exported for testing */
export function areMessagePropsEqual(prev: Props, next: Props): boolean {
  if (prev.message.uuid !== next.message.uuid) return false;
  // Only re-render on lastThinkingBlockId change if this message actually
  // has thinking content — otherwise every message in scrollback re-renders
  // whenever streaming thinking starts/stops (CC-941).
  if (prev.lastThinkingBlockId !== next.lastThinkingBlockId && hasThinkingContent(next.message as unknown as Parameters<typeof hasThinkingContent>[0])) {
    return false;
  }
  // Verbose toggle changes thinking block visibility/expansion
  if (prev.verbose !== next.verbose) return false;
  // Only re-render if this message's "is latest bash output" status changed,
  // not when the global latestBashOutputUUID changes to a different message
  const prevIsLatest = prev.latestBashOutputUUID === prev.message.uuid;
  const nextIsLatest = next.latestBashOutputUUID === next.message.uuid;
  if (prevIsLatest !== nextIsLatest) return false;
  if (prev.isTranscriptMode !== next.isTranscriptMode) return false;
  // containerWidth is an absolute number in the no-metadata path (wrapper
  // Box is skipped). Static messages must re-render on terminal resize.
  if (prev.containerWidth !== next.containerWidth) return false;
  if (prev.isStatic && next.isStatic) return true;
  return false;
}
export const Message = React.memo(MessageImpl, areMessagePropsEqual);
