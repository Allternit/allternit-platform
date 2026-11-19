import * as React from 'react';
import { useEffect, useRef, useState } from 'react';
import { useInterval } from 'usehooks-ts';
import type { CommandResultDisplay } from '../../commands';
import { Markdown } from '../../components/Markdown';
import { SpinnerGlyph } from '../../components/Spinner/SpinnerGlyph';
import { DOWN_ARROW, UP_ARROW } from '../../constants/figures';
import { getSystemPrompt } from '../../constants/prompts';
import { useModalOrTerminalSize } from '../../context/modalContext';
import { getSystemContext, getUserContext } from '../../context';
import { useTerminalSize } from '../../hooks/useTerminalSize';
import ScrollBox, { type ScrollBoxHandle } from '../../ink/components/ScrollBox';
import type { KeyboardEvent } from '../../ink/events/keyboard-event';
import { Box, Text } from '../../ink';
import type { LocalJSXCommandOnDone } from '../../types/command';
import type { Message } from '../../types/message';
import { createAbortController } from '../../utils/abortController';
import { saveGlobalConfig } from '../../utils/config';
import { errorMessage } from '../../utils/errors';
import { type CacheSafeParams, getLastCacheSafeParams } from '../../utils/forkedAgent';
import { getMessagesAfterCompactBoundary } from '../../utils/messages';
import type { ProcessUserInputContext } from '../../utils/processUserInput/processUserInput';
import { runSideQuestion } from '../../utils/sideQuestion';
import { asSystemPrompt } from '../../utils/systemPromptType';
type BtwComponentProps = {
  question: string;
  context: ProcessUserInputContext;
  onDone: (result?: string, options?: {
    display?: CommandResultDisplay;
  }) => void;
};
const CHROME_ROWS = 5;
const OUTER_CHROME_ROWS = 6;
const SCROLL_LINES = 3;
function BtwSideQuestion({
    question,
    context,
    onDone
}: BtwComponentProps) {
  const [response, setResponse] = useState(null);
  const [error, setError] = useState(null);
  const [frame, setFrame] = useState(0);
  const scrollRef = useRef(null);
  const {
    rows
  } = useModalOrTerminalSize(useTerminalSize());
  const t1 = () => setFrame(_temp);

  useInterval(t1, response || error ? null : 80);
  const t2 = function handleKeyDown(e) {
      if (e.key === "escape" || e.key === "return" || e.key === " " || e.ctrl && (e.key === "c" || e.key === "d")) {
        e.preventDefault();
        onDone(undefined, {
          display: "skip"
        });
        return;
      }
      if (e.key === "up" || e.ctrl && e.key === "p") {
        e.preventDefault();
        scrollRef.current?.scrollBy(-SCROLL_LINES);
      }
      if (e.key === "down" || e.ctrl && e.key === "n") {
        e.preventDefault();
        scrollRef.current?.scrollBy(SCROLL_LINES);
      }
    };

  const handleKeyDown = t2;
  const t3 = () => {
      const abortController = createAbortController();
      const fetchResponse = async function fetchResponse() {
        ;
        try {
          const cacheSafeParams = await buildCacheSafeParams(context);
          const result = await runSideQuestion({
            question,
            cacheSafeParams
          });
          if (!abortController.signal.aborted) {
            if (result.response) {
              setResponse(result.response);
            } else {
              setError("No response received");
            }
          }
        } catch (t5) {
          const err = t5;
          if (!abortController.signal.aborted) {
            setError(errorMessage(err) || "Failed to get response");
          }
        }
      };
      fetchResponse();
      return () => {
        abortController.abort();
      };
    };
  const t4 = [question, context];

  useEffect(t3, t4);
  const maxContentHeight = Math.max(5, rows - CHROME_ROWS - OUTER_CHROME_ROWS);
  const t5 = <Text color="warning" bold={true}>/btw{" "}</Text>;

  const t6 = <Box>{t5}<Text dimColor={true}>{question}</Text></Box>;

  const t7 = <ScrollBox ref={scrollRef} flexDirection="column" flexGrow={1}>{error ? <Text color="error">{error}</Text> : response ? <Markdown>{response}</Markdown> : <Box><SpinnerGlyph frame={frame} messageColor="warning" /><Text color="warning">Answering...</Text></Box>}</ScrollBox>;

  const t8 = <Box marginTop={1} marginLeft={2} maxHeight={maxContentHeight}>{t7}</Box>;

  const t9 = (response || error) && <Box marginTop={1}><Text dimColor={true}>{UP_ARROW}/{DOWN_ARROW} to scroll · Space, Enter, or Escape to dismiss</Text></Box>;

  const t10 = <Box flexDirection="column" paddingLeft={2} marginTop={1} tabIndex={0} autoFocus={true} onKeyDown={handleKeyDown}>{t6}{t8}{t9}</Box>;

  return t10;
}

/**
 * Build CacheSafeParams for the side question fork.
 *
 * The preferred source is getLastCacheSafeParams — the exact
 * systemPrompt/userContext/systemContext bytes the main thread sent on its
 * last request (captured in stopHooks). Reusing them guarantees a byte-
 * identical prefix and thus a prompt cache hit. We pair these with the
 * current toolUseContext (for thinkingConfig/tools) and current messages
 * (for up-to-date context).
 *
 * Fallback (first turn before stop hooks fire, or prompt-suggestion
 * disabled): rebuild from scratch. This may miss the cache if the main loop
 * applied buildEffectiveSystemPrompt extras (--agent, --system-prompt,
 * --append-system-prompt, coordinator mode).
 */
function _temp(f) {
  return f + 1;
}
function stripInProgressAssistantMessage(messages: Message[]): Message[] {
  const last = messages.at(-1);
  if (last?.type === 'assistant' && last.message.stop_reason === null) {
    return messages.slice(0, -1);
  }
  return messages;
}
async function buildCacheSafeParams(context: ProcessUserInputContext): Promise<CacheSafeParams> {
  const forkContextMessages = getMessagesAfterCompactBoundary(stripInProgressAssistantMessage(context.messages));
  const saved = getLastCacheSafeParams();
  if (saved) {
    return {
      systemPrompt: saved.systemPrompt,
      userContext: saved.userContext,
      systemContext: saved.systemContext,
      toolUseContext: context,
      forkContextMessages
    };
  }
  const [rawSystemPrompt, userContext, systemContext] = await Promise.all([getSystemPrompt(context.options.tools, context.options.mainLoopModel, [], context.options.mcpClients, context.getAppState().toolPermissionContext.mode), getUserContext(), getSystemContext()]);
  return {
    systemPrompt: asSystemPrompt(rawSystemPrompt),
    userContext,
    systemContext,
    toolUseContext: context,
    forkContextMessages
  };
}
export async function call(onDone: LocalJSXCommandOnDone, context: ProcessUserInputContext, args: string): Promise<React.ReactNode> {
  const question = args?.trim();
  if (!question) {
    onDone('Usage: /btw <your question>', {
      display: 'system'
    });
    return null;
  }
  saveGlobalConfig(current => ({
    ...current,
    btwUseCount: current.btwUseCount + 1
  }));
  return <BtwSideQuestion question={question} context={context} onDone={onDone} />;
}
