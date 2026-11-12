import { useCallback, useEffect, useRef, useState } from "react";
import { formatApiError } from "@/lib/api-client";
import { isCloudGateway } from "@/lib/availability";
import {
  ensureConsoleGatewayKey,
  forgetPlaygroundKey,
  forgetStoredGatewayKey,
  gatewayChat,
  GatewayHttpError,
  getPlaygroundKey,
  type ChatCompletionRequest,
  type ChatToolCall,
} from "@/lib/console-gateway";
import {
  assembledToJson,
  mergeChunk,
  newAssembly,
  readCompletion,
  type AssembledResponse,
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
  finishReason: string | null;
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
  request: null,
  responseJson: null,
  responseHeaders: null,
  status: null,
  latencyMs: null,
  requestId: null,
};

export interface ChatRunArgs {
  /** Full OpenAI-compatible request body (see buildRequestBody). */
  body: Record<string, unknown>;
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
 * One runnable chat-completion lane. Cloud: a 15-minute project key held in
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

  const stop = useCallback(() => abortRef.current?.abort(), []);
  const reset = useCallback(() => {
    abortRef.current?.abort();
    setState(IDLE);
  }, []);

  const run = useCallback(async (args: ChatRunArgs) => {
    abortRef.current?.abort();
    const abort = new AbortController();
    abortRef.current = abort;
    const request = args.body as unknown as ChatCompletionRequest;
    setState({ ...IDLE, running: true, request: args.body });

    let acc: AssembledResponse = newAssembly();
    const attempt = async (force: boolean) => {
      acc = newAssembly();
      const key = await acquireKey(args.projectId, force);
      return gatewayChat(key, request, {
        signal: abort.signal,
        onChunk: (chunk) => {
          acc = mergeChunk(acc, chunk);
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
          if (isCloudGateway()) forgetPlaygroundKey();
          result = await attempt(true);
        } else {
          throw err;
        }
      }
      const final = request.stream ? acc : readCompletion(result.json);
      setState({
        ...IDLE,
        text: final.content,
        toolCalls: final.toolCalls,
        usage: final.usage,
        modelUsed: final.model ?? null,
        finishReason: final.finishReason,
        request: args.body,
        responseJson: request.stream ? assembledToJson(final) : result.json,
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
  }, []);

  useEffect(() => () => abortRef.current?.abort(), []);

  return { state, run, stop, reset };
}
