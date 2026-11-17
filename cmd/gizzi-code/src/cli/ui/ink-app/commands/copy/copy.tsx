import { mkdir, writeFile } from 'fs/promises';
import { marked, type Tokens } from 'marked';
import { tmpdir } from 'os';
import { join } from 'path';
import React, { useRef } from 'react';
import type { CommandResultDisplay } from '../../commands';
import type { OptionWithDescription } from '../../components/CustomSelect/select';
import { Select } from '../../components/CustomSelect/select';
import { Byline } from '../../components/design-system/Byline';
import { KeyboardShortcutHint } from '../../components/design-system/KeyboardShortcutHint';
import { Pane } from '../../components/design-system/Pane';
import type { KeyboardEvent } from '../../ink/events/keyboard-event';
import { stringWidth } from '../../ink/stringWidth';
import { setClipboard } from '../../ink/termio/osc';
import { Box, Text } from '../../ink';
import { logEvent } from '../../services/analytics/index';
import type { LocalJSXCommandCall } from '../../types/command';
import type { AssistantMessage, Message } from '../../types/message';
import { getGlobalConfig, saveGlobalConfig } from '../../utils/config';
import { extractTextContent } from '../../utils/messages';
import { stripPromptXMLTags } from '../../utils/syntheticMessages.js';
import { countCharInString } from '../../utils/stringUtils';
import { parseWritingDraft, writingCopyText } from '../../tools/LookupTool/WritingDraft';
const COPY_DIR = join(tmpdir(), 'claude');
const RESPONSE_FILENAME = 'response.md';
const MAX_LOOKBACK = 20;
type CodeBlock = {
  code: string;
  lang: string | undefined;
  /** Set for a writing draft: the picker shows this instead of the code's first line. */
  label?: string;
};
function extractCodeBlocks(markdown: string): CodeBlock[] {
  const tokens = marked.lexer(stripPromptXMLTags(markdown));
  const blocks: CodeBlock[] = [];
  for (const token of tokens) {
    if (token.type === 'code') {
      const codeToken = token as Tokens.Code;
      // A writing answer card copies as readable text (To, Subject, body, alternatives), never its JSON.
      const draft = parseWritingDraft(codeToken.lang, codeToken.text);
      blocks.push(draft ? {
        code: writingCopyText(draft),
        lang: undefined,
        label: `Draft: ${draft.title}`
      } : {
        code: codeToken.text,
        lang: codeToken.lang
      });
    }
  }
  return blocks;
}

/** The response with each writing draft fence replaced by its readable text; unchanged when it has none. */
function readableResponse(markdown: string): string {
  const tokens = marked.lexer(stripPromptXMLTags(markdown));
  let changed = false;
  const parts = tokens.map(token => {
    if (token.type !== 'code') return token.raw;
    const draft = parseWritingDraft((token as Tokens.Code).lang, (token as Tokens.Code).text);
    if (!draft) return token.raw;
    changed = true;
    const trailing = /\n*$/.exec(token.raw)?.[0] ?? '';
    return writingCopyText(draft) + trailing;
  });
  return changed ? parts.join('') : markdown;
}

/**
 * Walk messages newest-first, returning text from assistant messages that
 * actually said something (skips tool-use-only turns and API errors).
 * Index 0 = latest, 1 = second-to-latest, etc. Caps at MAX_LOOKBACK.
 */
export function collectRecentAssistantTexts(messages: Message[]): string[] {
  const texts: string[] = [];
  for (let i = messages.length - 1; i >= 0 && texts.length < MAX_LOOKBACK; i--) {
    const msg = messages[i];
    if (msg?.type !== 'assistant' || msg.isApiErrorMessage) continue;
    const content = (msg as AssistantMessage).message.content;
    if (!Array.isArray(content)) continue;
    const text = extractTextContent(content, '\n\n');
    if (text) texts.push(text);
  }
  return texts;
}
export function fileExtension(lang: string | undefined): string {
  if (lang) {
    // Sanitize to prevent path traversal (e.g. ```../../etc/passwd)
    // Language identifiers are alphanumeric: python, tsx, jsonc, etc.
    const sanitized = lang.replace(/[^a-zA-Z0-9]/g, '');
    if (sanitized && sanitized !== 'plaintext') {
      return `.${sanitized}`;
    }
  }
  return '.txt';
}
async function writeToFile(text: string, filename: string): Promise<string> {
  const filePath = join(COPY_DIR, filename);
  await mkdir(COPY_DIR, {
    recursive: true
  });
  await writeFile(filePath, text, 'utf-8');
  return filePath;
}
async function copyOrWriteToFile(text: string, filename: string): Promise<string> {
  const raw = await setClipboard(text);
  if (raw) process.stdout.write(raw);
  const lineCount = countCharInString(text, '\n') + 1;
  const charCount = text.length;
  // Also write to a temp file — clipboard paths are best-effort (OSC 52 needs
  // terminal support), so the file provides a reliable fallback.
  try {
    const filePath = await writeToFile(text, filename);
    return `Copied to clipboard (${charCount} characters, ${lineCount} lines)\nAlso written to ${filePath}`;
  } catch {
    return `Copied to clipboard (${charCount} characters, ${lineCount} lines)`;
  }
}
function truncateLine(text: string, maxLen: number): string {
  const firstLine = text.split('\n')[0] ?? '';
  if (stringWidth(firstLine) <= maxLen) {
    return firstLine;
  }
  let result = '';
  let width = 0;
  const targetWidth = maxLen - 1;
  for (const char of firstLine) {
    const charWidth = stringWidth(char);
    if (width + charWidth > targetWidth) break;
    result += char;
    width += charWidth;
  }
  return result + '\u2026';
}
type PickerProps = {
  fullText: string;
  codeBlocks: CodeBlock[];
  messageAge: number;
  onDone: (result?: string, options?: {
    display?: CommandResultDisplay;
  }) => void;
};
type PickerSelection = number | 'full' | 'always';
function CopyPicker({
    fullText,
    codeBlocks,
    messageAge,
    onDone
}: PickerProps) {
  const focusedRef = useRef("full");
  const t1 = `${fullText.length} chars, ${countCharInString(fullText, "\n") + 1} lines`;
  const t2 = {
      label: "Full response",
      value: "full" as const,
      description: t1
    };

  const t4 = {
        label: "Always copy full response",
        value: "always" as const,
        description: "Skip this picker in the future (revert via /config)"
      };

  const t3 = [t2, ...codeBlocks.map(_temp), t4];

  const options = t3;
  const t4_2 = function getSelectionContent(selected) {
      if (selected === "full" || selected === "always") {
        return {
          text: fullText,
          filename: RESPONSE_FILENAME
        };
      }
      const block_0 = codeBlocks[selected];
      return {
        text: block_0.code,
        filename: `copy${fileExtension(block_0.lang)}`,
        blockIndex: selected
      };
    };

  const getSelectionContent = t4_2;
  const t5 = async function handleSelect(selected_0) {
      const content = getSelectionContent(selected_0);
      if (selected_0 === "always") {
        if (!getGlobalConfig().copyFullResponse) {
          saveGlobalConfig(_temp2);
        }
        logEvent("tengu_copy", {
          block_count: codeBlocks.length,
          always: true,
          message_age: messageAge
        });
        const result = await copyOrWriteToFile(content.text, content.filename);
        onDone(`${result}\nPreference saved. Use /config to change copyFullResponse`);
        return;
      }
      logEvent("tengu_copy", {
        selected_block: content.blockIndex,
        block_count: codeBlocks.length,
        message_age: messageAge
      });
      const result_0 = await copyOrWriteToFile(content.text, content.filename);
      onDone(result_0);
    };

  const handleSelect = t5;
  const handleWrite = async function handleWrite(selected_1) {
      const content_0 = getSelectionContent(selected_1);
      logEvent("tengu_copy", {
        selected_block: content_0.blockIndex,
        block_count: codeBlocks.length,
        message_age: messageAge,
        write_shortcut: true
      });
      ;
      try {
        const filePath = await writeToFile(content_0.text, content_0.filename);
        onDone(`Written to ${filePath}`);
      } catch (t7) {
        const e = t7;
        onDone(`Failed to write file: ${e instanceof Error ? e.message : e}`);
      }
    };
  const t6 = function handleKeyDown(e_0) {
      if (e_0.key === "w") {
        e_0.preventDefault();
        handleWrite(focusedRef.current);
      }
    };

  const handleKeyDown = t6;
  const t7 = <Text dimColor={true}>Select content to copy:</Text>;

  const t8 = value => {
      focusedRef.current = value;
    };

  const t9 = selected_2 => {
      handleSelect(selected_2);
    };

  const t10 = () => {
      onDone("Copy cancelled", {
        display: "system"
      });
    };

  const t11 = <Select options={options} hideIndexes={false} onFocus={t8} onChange={t9} onCancel={t10} />;

  const t12 = <Text dimColor={true}><Byline><KeyboardShortcutHint shortcut="enter" action="copy" /><KeyboardShortcutHint shortcut="w" action="write to file" /><KeyboardShortcutHint shortcut="esc" action="cancel" /></Byline></Text>;

  const t13 = <Pane><Box flexDirection="column" gap={1} tabIndex={0} autoFocus={true} onKeyDown={handleKeyDown}>{t7}{t11}{t12}</Box></Pane>;

  return t13;
}
function _temp2(c) {
  return {
    ...c,
    copyFullResponse: true
  };
}
function _temp(block, index) {
  const blockLines = countCharInString(block.code, "\n") + 1;
  return {
    label: block.label ? truncateLine(block.label, 60) : truncateLine(block.code, 60),
    value: index,
    description: [block.label ? 'writing draft' : block.lang, blockLines > 1 ? `${blockLines} lines` : undefined].filter(Boolean).join(", ") || undefined
  };
}
export const call: LocalJSXCommandCall = async (onDone, context, args) => {
  const texts = collectRecentAssistantTexts(context.messages);
  if (texts.length === 0) {
    onDone('No assistant message to copy');
    return null;
  }

  // /copy N reaches back N-1 messages (1 = latest, 2 = second-to-latest, ...)
  let age = 0;
  const arg = args?.trim();
  if (arg) {
    const n = Number(arg);
    if (!Number.isInteger(n) || n < 1) {
      onDone(`Usage: /copy [N] where N is 1 (latest), 2, 3, \u2026 Got: ${arg}`);
      return null;
    }
    if (n > texts.length) {
      onDone(`Only ${texts.length} assistant ${texts.length === 1 ? 'message' : 'messages'} available to copy`);
      return null;
    }
    age = n - 1;
  }
  const codeBlocks = extractCodeBlocks(texts[age]!);
  const text = readableResponse(texts[age]!);
  const config = getGlobalConfig();
  if (codeBlocks.length === 0 || config.copyFullResponse) {
    logEvent('tengu_copy', {
      always: config.copyFullResponse,
      block_count: codeBlocks.length,
      message_age: age
    });
    const result = await copyOrWriteToFile(text, RESPONSE_FILENAME);
    onDone(result);
    return null;
  }
  return <CopyPicker fullText={text} codeBlocks={codeBlocks} messageAge={age} onDone={onDone} />;
};
