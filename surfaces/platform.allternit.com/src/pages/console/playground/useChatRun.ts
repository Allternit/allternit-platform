import { useCallback, useEffect, useRef, useState } from "react";
import { formatApiError } from "@/lib/api-client";
import { isCloudGateway } from "@/lib/availability";
import { usePlatformAuth } from "@/lib/platform-auth-client";
import { PROJECT_HEADER } from "@/lib/platform-v1";
import {
  ensureConsoleGatewayKey,
  forgetPlaygroundKey,
  forgetStoredGatewayKey,
  gatewayPost,
  GatewayHttpError,
  getPlaygroundKey,
  type ChatToolCall,
} from "@/lib/console-gateway";
import {
  MODE_PATHS,
  mergeModeEvent,
  modeAssemblyToJson,
  newModeAssembly,
  readModeReply,
  type ModeAssembly,
  type PlaygroundMode,
} from "@/lib/playground-request";

export interface ChatRunUsage {
  prompt_tokens?: number;
  completion_tokens?: number;
  total_tokens?: number;
}

export interface ChatRunState {
  running: boolean;
  text: string;
  error: string | null;
  usage: ChatRunUsage | null;
  modelUsed: string | null;
  stopped: boolean;
  toolCalls: ChatToolCall[];
  /** Chat finish_reason, Responses status, or Messages stop_reason. */
  finishReason: string | null;
  /** The route the last run used. */
  mode: PlaygroundMode;
  /** Responses `incomplete_details.reason`. */
  incompleteReason: string | null;
  /** The reply id (`chatcmpl_…`, `resp_…`, `mdl_…`). */
  responseId: string | null;
  /** Inspector data. */
  request: Record<string, unknown> | null;
  responseJson: unknown;
  responseHeaders: Record<string, string> | null;
  status: number | null;
  latencyMs: number | null;
  requestId: string | null;
}

const IDLE: ChatRunState = {
  running: false,
  text: "",
  error: null,
  usage: null,
  modelUsed: null,
  stopped: false,
  toolCalls: [],
  finishReason: null,
  mode: "chat",
  incompleteReason: null,
  responseId: null,
  request: null,
  responseJson: null,
  responseHeaders: null,
  status: null,
  latencyMs: null,
  requestId: null,
};

export interface ChatRunArgs {
  /** Full request body for `mode` (see buildModeRequestBody). */
  body: Record<string, unknown>;
  /** Route to call; chat completions when omitted. */
  mode?: PlaygroundMode;
  /** Selected project; required on cloud to mint the playground key. */
  projectId: string | null;
}

async function acquireKey(projectId: string | null, force: boolean): Promise<string> {
  if (isCloudGateway()) {
    if (!projectId) throw new Error("Select a project to use the Playground.");
    return getPlaygroundKey(projectId, force);
  }
  if (force) forgetStoredGatewayKey();
  return ensureConsoleGatewayKey();
}

/**
 * One runnable Playground lane (chat completions, Responses or Messages). Cloud: a 15-minute project key held in
 * memory (re-minted on expiry or 401). Runtime: the stored console key.
 * `stop()` aborts the stream. Compare lanes are independent instances.
 */
export function useChatRun(): {
  state: ChatRunState;
  run: (args: ChatRunArgs) => Promise<void>;
  stop: () => void;
  reset: () => void;
} {
  const [state, setState] = useState<ChatRunState>(IDLE);
  const abortRef = useRef<AbortController | null>(null);
  const auth = usePlatformAuth();
  const getSessionToken = auth?.getToken;

  const stop = useCallback(() => abortRef.current?.abort(), []);
  const reset = useCallback(() => {
    abortRef.current?.abort();
    setState(IDLE);
  }, []);

  const run = useCallback(async (args: ChatRunArgs) => {
    abortRef.current?.abort();
    const abort = new AbortController();
    abortRef.current = abort;
    const mode: PlaygroundMode = args.mode ?? "chat";
    const streaming = args.body.stream === true;
    setState({ ...IDLE, running: true, mode, request: args.body });

    let acc: ModeAssembly = newModeAssembly();
    // On cloud, /v1/responses and /v1/messages are Platform API routes: they take a
    // project key (alt_live_/alt_test_) or the console session plus the project
    // header, never the user-style playground key (that one is for chat completions).
    // The session path bills the selected project, which these routes always do.
    const sessionMode = isCloudGateway() && mode !== "chat";
    const attempt = async (force: boolean) => {
      acc = newModeAssembly();
      let key: string;
      let headers: Record<string, string> | undefined;
      if (sessionMode) {
        if (!args.projectId) throw new Error("Select a project to use the Playground.");
        const token = getSessionToken ? await getSessionToken() : null;
        if (!token) throw new Error("Your console session has ended. Sign in again, then run.");
        key = token;
        headers = { [PROJECT_HEADER]: args.projectId };
      } else {
        key = await acquireKey(args.projectId, force);
      }
      return gatewayPost(MODE_PATHS[mode], key, args.body, {
        signal: abort.signal,
        headers,
        onEvent: (event, data) => {
          acc = mergeModeEvent(mode, acc, event, data);
          setState((prev) => ({
            ...prev,
            text: acc.content,
            toolCalls: acc.toolCalls,
            modelUsed: acc.model ?? prev.modelUsed,
          }));
        },
      });
    };

    try {
      let result;
      try {
        result = await attempt(false);
      } catch (err) {
        if (err instanceof GatewayHttpError && err.status === 401) {
          if (isCloudGateway() && !sessionMode) forgetPlaygroundKey();
          result = await attempt(true);
        } else {
          throw err;
        }
      }
      const final = streaming ? acc : readModeReply(mode, result.json);
      setState({
        ...IDLE,
        text: final.content,
        toolCalls: final.toolCalls,
        usage: final.usage,
        modelUsed: final.model ?? null,
        finishReason: final.finishReason,
        mode,
        incompleteReason: final.incompleteReason,
        responseId: final.id ?? null,
        error: final.error,
        request: args.body,
        responseJson: streaming ? modeAssemblyToJson(mode, final) : result.json,
        responseHeaders: result.headers,
        status: result.status,
        latencyMs: result.latencyMs,
        requestId: result.headers["x-request-id"] ?? null,
      });
    } catch (err) {
      if (err instanceof Error && err.name === "AbortError") {
        setState((prev) => ({ ...prev, running: false, stopped: true }));
        return;
      }
      setState((prev) => ({
        ...prev,
        running: false,
        error: formatApiError(err, "Request failed"),
        status: err instanceof GatewayHttpError ? err.status : prev.status,
        responseHeaders: err instanceof GatewayHttpError ? err.headers : prev.responseHeaders,
        responseJson: err instanceof GatewayHttpError ? err.body : prev.responseJson,
        latencyMs: err instanceof GatewayHttpError ? err.latencyMs : prev.latencyMs,
        requestId: err instanceof GatewayHttpError ? (err.headers["x-request-id"] ?? null) : prev.requestId,
      }));
    } finally {
      if (abortRef.current === abort) abortRef.current = null;
    }
  }, [getSessionToken]);

  useEffect(() => () => abortRef.current?.abort(), []);

  return { state, run, stop, reset };
}
