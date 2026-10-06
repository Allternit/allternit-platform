import React, { useEffect, useState } from 'react';
import type { CommandResultDisplay } from '@/commands.js';
// eslint-disable-next-line custom-rules/prefer-use-keybindings -- raw input for "any key" dismiss and y/n prompt
import { Box, Text, useInput } from '@/ink.js';
import { openBrowser } from '../../../../shared/utils/browser.js';
import { getDesktopInstallStatus, openCurrentSessionInDesktop } from '../../../../shared/utils/desktopDeepLink.js';
import { errorMessage } from '../../../../shared/utils/errors.js';
import { gracefulShutdown } from '../../../../shared/utils/gracefulShutdown.js';
import { flushSessionStorage } from '../../../../shared/utils/sessionStorage.js';
import { LoadingState } from './design-system/LoadingState.js';
const DESKTOP_DOCS_URL = 'https://docs.allternit.com/desktop';
export function getDownloadUrl(): string {
  return 'https://github.com/Allternit/desktop/releases';
}
type DesktopHandoffState = 'checking' | 'prompt-download' | 'flushing' | 'opening' | 'success' | 'error';
type Props = {
  onDone: (result?: string, options?: {
    display?: CommandResultDisplay;
  }) => void;
};
export function DesktopHandoff({
    onDone
}: Props) {
  const [state, setState] = useState("checking");
  const [error, setError] = useState(null);
  const [downloadMessage, setDownloadMessage] = useState("");
  const t1 = input => {
      if (state === "error") {
        onDone(error ?? "Unknown error", {
          display: "system"
        });
        return;
      }
      if (state === "prompt-download") {
        if (input === "y" || input === "Y") {
          openBrowser(getDownloadUrl()).catch(_temp);
          onDone(`Starting download. Re-run /desktop once you\u2019ve installed the app.\nLearn more at ${DESKTOP_DOCS_URL}`, {
            display: "system"
          });
        } else {
          if (input === "n" || input === "N") {
            onDone(`The desktop app is required for /desktop. Learn more at ${DESKTOP_DOCS_URL}`, {
              display: "system"
            });
          }
        }
      }
    };

  useInput(t1);
  const t2 = () => {
      const performHandoff = async function performHandoff() {
        setState("checking");
        const installStatus = await getDesktopInstallStatus();
        if (installStatus.status === "not-installed") {
          setDownloadMessage("Allternit Desktop is not installed.");
          setState("prompt-download");
          return;
        }
        if (installStatus.status === "version-too-old") {
          setDownloadMessage(`Allternit Desktop needs to be updated (found v${installStatus.version}, need v1.1.2396+).`);
          setState("prompt-download");
          return;
        }
        setState("flushing");
        await flushSessionStorage();
        setState("opening");
        const result = await openCurrentSessionInDesktop();
        if (!result.success) {
          setError(result.error ?? "Failed to open Allternit Desktop");
          setState("error");
          return;
        }
        setState("success");
        setTimeout(_temp2, 500, onDone);
      };
      performHandoff().catch(err => {
        setError(errorMessage(err));
        setState("error");
      });
    };
  const t3 = [onDone];

  useEffect(t2, t3);
  if (state === "error") {
    const t4 = <Text color="error">Error: {error}</Text>;

    const t5 = <Text dimColor={true}>Press any key to continue…</Text>;

    const t6 = <Box flexDirection="column" paddingX={2}>{t4}{t5}</Box>;

    return t6;
  }
  if (state === "prompt-download") {
    const t4 = <Text>{downloadMessage}</Text>;

    const t5 = <Text>Download now? (y/n)</Text>;

    const t6 = <Box flexDirection="column" paddingX={2}>{t4}{t5}</Box>;

    return t6;
  }
  const t4 = {
      checking: "Checking for Allternit Desktop\u2026",
      flushing: "Saving session\u2026",
      opening: "Opening Allternit Desktop\u2026",
      success: "Opening in Allternit Desktop\u2026"
    };

  const messages = t4;
  const t5 = messages[state];
  const t6 = <LoadingState message={t5} />;

  return t6;
}
async function _temp2(onDone_0) {
  onDone_0("Session transferred to Allternit Desktop", {
    display: "system"
  });
  await gracefulShutdown(0, "other");
}
function _temp() {}
