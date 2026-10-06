import React, { useEffect, useRef, useState } from 'react';
import type { CommandResultDisplay } from '../../commands';
import { Box, Text } from '@/ink.js';
import {
  buildDesignDeepLink,
  openDesignStudioInDesktop,
} from '../../../../../shared/utils/desktopDeepLink.js';
import { readDesignAck } from '../../../../../shared/utils/designPromptAck.js';
import { errorMessage } from '../../../../../shared/utils/errors.js';
import { LoadingState } from '../../components/design-system/LoadingState.js';

const DESKTOP_DOWNLOAD_URL = 'https://github.com/Allternit/desktop/releases';

// How long the command waits for the studio to report prompt consumption
// (~/.allternit/design-prompt-ack.json, written via POST /v1/design/ack).
const ACK_POLL_INTERVAL_MS = 500;
const ACK_POLL_TIMEOUT_MS = 8_000;

type Props = {
  prompt: string;
  onDone: (
    result?: string,
    options?: { display?: CommandResultDisplay },
  ) => void;
};

function DesignOpen({ prompt, onDone }: Props): React.ReactNode {
  const [error, setError] = useState<string | null>(null);
  const [notInstalled, setNotInstalled] = useState(false);
  const [waitingForAck, setWaitingForAck] = useState(false);
  const startTimeRef = useRef<number>(Date.now());
  const doneRef = useRef(false);

  const finish = (message: string) => {
    if (doneRef.current) return;
    doneRef.current = true;
    onDone(message, { display: 'system' });
  };

  useEffect(() => {
    let cancelled = false;
    let pollTimer: ReturnType<typeof setTimeout> | undefined;
    (async () => {
      try {
        const result = await openDesignStudioInDesktop(prompt);
        if (cancelled) return;
        if (result.success) {
          if (!prompt) {
            finish('A:// Studio opened in Allternit Desktop.\nDeep link: ' + result.deepLinkUrl);
            return;
          }
          // Poll the local ack receipt so the user learns whether the studio
          // actually consumed the prompt, not just that the deep link fired.
          setWaitingForAck(true);
          const deadline = startTimeRef.current + ACK_POLL_TIMEOUT_MS;
          const poll = async () => {
            if (cancelled || doneRef.current) return;
            const ack = await readDesignAck().catch(() => undefined);
            const fresh = ack && Date.parse(ack.consumedAt) >= startTimeRef.current - 2_000;
            if (ack && fresh && ack.prompt.trim() === prompt.trim()) {
              finish(
                `A:// Studio picked up your prompt.\nDeep link: ${result.deepLinkUrl}`,
              );
              return;
            }
            if (Date.now() >= deadline) {
              finish(
                `A:// Studio opened in Allternit Desktop with your prompt.\n` +
                  `The studio has not confirmed pickup yet — the prompt is still seeded in the composer when the design window loads.\n` +
                  `Deep link: ${result.deepLinkUrl}`,
              );
              return;
            }
            pollTimer = setTimeout(() => void poll(), ACK_POLL_INTERVAL_MS);
          };
          void poll();
          return;
        }
        if (result.notInstalled) {
          setNotInstalled(true);
          setError(
            `Allternit Desktop is not installed.\nDeep link: ${result.deepLinkUrl}\nInstall it from ${DESKTOP_DOWNLOAD_URL} and re-run /design.`,
          );
        } else {
          setError(`${result.error ?? 'Failed to open Allternit Desktop.'}\nDeep link: ${result.deepLinkUrl}`);
        }
      } catch (err) {
        if (!cancelled) {
          setError(`${errorMessage(err)}\nDeep link: ${buildDesignDeepLink(prompt)}`);
        }
      }
    })();
    return () => {
      cancelled = true;
      if (pollTimer) clearTimeout(pollTimer);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  if (error) {
    return (
      <Box flexDirection="column" paddingX={2}>
        <Text color={notInstalled ? 'yellow' : 'error'}>{error}</Text>
        <Text dimColor>Press any key to continue…</Text>
      </Box>
    );
  }

  return (
    <LoadingState
      message={
        waitingForAck
          ? 'Studio opened — waiting for it to pick up your prompt…'
          : prompt
            ? 'Opening A:// Studio with your prompt…'
            : 'Opening A:// Studio…'
      }
    />
  );
}

export async function call(
  onDone: (
    result?: string,
    options?: { display?: CommandResultDisplay },
  ) => void,
  _context: unknown,
  args: string,
): Promise<React.ReactNode> {
  return <DesignOpen prompt={(args ?? '').trim()} onDone={onDone} />;
}
