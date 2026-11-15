import * as React from 'react';
import { useInterval } from 'usehooks-ts';
import { getIsRemoteMode, getIsScrollDraining } from '../../bootstrap/state';
import { useNotifications } from '../../../../../context/notifications';
import { Text } from '../../ink';
import { getInitializationStatus, getLspServerManager } from '../../services/lsp/manager';
import { useSetAppState } from '../../state/AppState';
import { logForDebugging } from '../../utils/debug';
import { isEnvTruthy } from '../../utils/envUtils';
const LSP_POLL_INTERVAL_MS = 5000;

/**
 * Hook that polls LSP status and shows a notification when:
 * 1. Manager initialization fails
 * 2. Any LSP server enters an error state
 *
 * Also adds errors to appState.plugins.errors for /doctor display.
 *
 * Always on in the TUI (built-in servers for diagnostics after edits).
 */
export function useLspInitializationNotification() {
  const {
    addNotification
  } = useNotifications();
  const setAppState = useSetAppState();
  const [shouldPoll, setShouldPoll] = React.useState(_temp);
  const t0 = new Set();

  const notifiedErrorsRef = React.useRef(t0);
  const t1 = (source, errorMessage) => {
      const errorKey = `${source}:${errorMessage}`;
      if (notifiedErrorsRef.current.has(errorKey)) {
        return;
      }
      notifiedErrorsRef.current.add(errorKey);
      logForDebugging(`LSP error: ${source} - ${errorMessage}`);
      setAppState(prev => {
        const existingKeys = new Set(prev.plugins.errors.map(_temp2));
        const stateErrorKey = `generic-error:${source}:${errorMessage}`;
        if (existingKeys.has(stateErrorKey)) {
          return prev;
        }
        return {
          ...prev,
          plugins: {
            ...prev.plugins,
            errors: [...prev.plugins.errors, {
              type: "generic-error" as const,
              source,
              error: errorMessage
            }]
          }
        };
      });
      const displayName = source.startsWith("plugin:") || source.startsWith("builtin:") ? source.split(":")[1] ?? source : source;
      addNotification({
        key: `lsp-error-${source}`,
        jsx: <><Text color="error">LSP for {displayName} failed</Text><Text dimColor={true}> · /plugin for details</Text></>,
        priority: "medium",
        timeoutMs: 8000
      });
    };

  const addError = t1;
  const t2 = () => {
      if (getIsRemoteMode()) {
        return;
      }
      if (getIsScrollDraining()) {
        return;
      }
      const status = getInitializationStatus();
      if (status.status === "failed") {
        addError("lsp-manager", status.error.message);
        setShouldPoll(false);
        return;
      }
      if (status.status === "pending" || status.status === "not-started") {
        return;
      }
      const manager = getLspServerManager();
      if (manager) {
        const servers = manager.getAllServers();
        for (const [serverName, server] of servers) {
          if (server.state === "error" && server.lastError) {
            addError(serverName, server.lastError.message);
          }
        }
      }
    };

  const poll = t2;
  useInterval(poll, shouldPoll ? LSP_POLL_INTERVAL_MS : null);
  const t3 = () => {
      if (getIsRemoteMode() || !shouldPoll) {
        return;
      }
      poll();
    };
  const t4 = [poll, shouldPoll];

  React.useEffect(t3, t4);
}
function _temp2(e) {
  if (e.type === "generic-error") {
    return `generic-error:${e.source}:${e.error}`;
  }
  return `${e.type}:${e.source}`;
}
function _temp() {
  return isEnvTruthy("true");
}
