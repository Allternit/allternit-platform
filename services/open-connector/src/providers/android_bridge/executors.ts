import type { CredentialValidationResult, CredentialValidators, ProviderExecutors } from "../../core/types.ts";
import type { ExecutionContext } from "../../core/types.ts";

import { optionalString, requiredString } from "../../core/cast.ts";
import { defineProviderExecutors, ProviderRequestError, requireCustomCredential } from "../provider-runtime.ts";
import type { ProviderFetch } from "../provider-runtime.ts";

interface AndroidBridgeCredentials {
  baseUrl: string;
  deviceId?: string;
}

interface AndroidBridgeContext {
  values: Record<string, string>;
  fetcher: ProviderFetch;
}

function parseCredentials(values: Record<string, string>): AndroidBridgeCredentials {
  return {
    baseUrl: requiredString(values.baseUrl, "baseUrl", (m) => new ProviderRequestError(400, m)),
    deviceId: values.deviceId?.trim() || undefined,
  };
}

function buildUrl(creds: AndroidBridgeCredentials, path: string, query?: Record<string, string>): URL {
  const base = creds.baseUrl.endsWith("/") ? creds.baseUrl : `${creds.baseUrl}/`;
  const url = new URL(path.slice(1), base);
  if (creds.deviceId) {
    url.searchParams.set("device_id", creds.deviceId);
  }
  if (query) {
    for (const [key, value] of Object.entries(query)) {
      url.searchParams.set(key, value);
    }
  }
  return url;
}

async function requestJson<T>(
  context: AndroidBridgeContext,
  creds: AndroidBridgeCredentials,
  path: string,
  options?: {
    method?: "GET" | "POST";
    body?: Record<string, unknown>;
    query?: Record<string, string>;
  },
): Promise<T> {
  const url = buildUrl(creds, path, options?.query);
  const init: RequestInit = {
    method: options?.method ?? "GET",
    headers: { accept: "application/json" },
  };
  if (options?.body) {
    init.body = JSON.stringify(options.body);
    (init.headers as Record<string, string>)["content-type"] = "application/json";
  }

  let response: Response;
  try {
    response = await context.fetcher(url, init);
  } catch (err) {
    throw new ProviderRequestError(
      502,
      err instanceof Error ? `Android Bridge unreachable: ${err.message}` : "Android Bridge unreachable",
    );
  }

  if (!response.ok) {
    const text = await response.text().catch(() => "");
    throw new ProviderRequestError(response.status, text || `Android Bridge request failed with HTTP ${response.status}`);
  }

  try {
    return (await response.json()) as T;
  } catch (err) {
    throw new ProviderRequestError(
      502,
      err instanceof Error ? `Invalid JSON from Android Bridge: ${err.message}` : "Invalid JSON from Android Bridge",
    );
  }
}

export async function validateAndroidBridgeCredential(
  input: { values: Record<string, string> },
): Promise<CredentialValidationResult> {
  const creds = parseCredentials(input.values);
  const result = await requestJson<{ ok: boolean; ready: boolean }>(
    { values: input.values, fetcher: fetch },
    creds,
    "/health",
  );
  if (!result.ok || result.ready !== true) {
    throw new ProviderRequestError(400, "Android Bridge is not ready or no device is connected.");
  }
  return {
    profile: {
      accountId: creds.baseUrl,
      displayName: `Android Bridge (${creds.baseUrl})`,
      grantedScopes: [],
    },
    grantedScopes: [],
    metadata: {
      baseUrl: creds.baseUrl,
      deviceId: creds.deviceId,
    },
  };
}

async function sendSms(input: Record<string, unknown>, context: AndroidBridgeContext): Promise<unknown> {
  const creds = parseCredentials(context.values);
  const to = requiredString(input.to, "to", (m) => new ProviderRequestError(400, m));
  const body = requiredString(input.body, "body", (m) => new ProviderRequestError(400, m));
  return requestJson<{ sent: boolean; to: string }>(context, creds, "/send-sms", {
    method: "POST",
    body: { to, body },
  });
}

async function listMessages(input: Record<string, unknown>, context: AndroidBridgeContext): Promise<unknown> {
  const creds = parseCredentials(context.values);
  const limit = Math.min(Math.max(Number(input.limit) || 20, 1), 100);
  return requestJson<{ messages: Record<string, unknown>[] }>(context, creds, "/messages", {
    query: { limit: String(limit) },
  });
}

async function screenshot(_input: Record<string, unknown>, context: AndroidBridgeContext): Promise<unknown> {
  const creds = parseCredentials(context.values);
  return requestJson<{ image: string; format: string }>(context, creds, "/screenshot");
}

async function tap(input: Record<string, unknown>, context: AndroidBridgeContext): Promise<unknown> {
  const creds = parseCredentials(context.values);
  const text = optionalString(input.text);
  const x = input.x === undefined ? undefined : Number(input.x);
  const y = input.y === undefined ? undefined : Number(input.y);

  if (!text && (x === undefined || y === undefined || Number.isNaN(x) || Number.isNaN(y))) {
    throw new ProviderRequestError(400, "Provide 'text' to tap or 'x' and 'y' coordinates.");
  }

  const body: Record<string, unknown> = {};
  if (text) body.text = text;
  if (x !== undefined && !Number.isNaN(x)) body.x = x;
  if (y !== undefined && !Number.isNaN(y)) body.y = y;

  return requestJson<{ tapped: boolean; text?: string; x?: number; y?: number }>(context, creds, "/tap", {
    method: "POST",
    body,
  });
}

async function typeText(input: Record<string, unknown>, context: AndroidBridgeContext): Promise<unknown> {
  const creds = parseCredentials(context.values);
  const text = requiredString(input.text, "text", (m) => new ProviderRequestError(400, m));
  return requestJson<{ typed: boolean; length: number }>(context, creds, "/type", {
    method: "POST",
    body: { text },
  });
}

async function pressKey(input: Record<string, unknown>, context: AndroidBridgeContext): Promise<unknown> {
  const creds = parseCredentials(context.values);
  const key = requiredString(input.key, "key", (m) => new ProviderRequestError(400, m));
  return requestJson<{ pressed: string }>(context, creds, "/press-key", {
    method: "POST",
    body: { key },
  });
}

const actionHandlers: Record<string, (input: Record<string, unknown>, context: AndroidBridgeContext) => Promise<unknown>> = {
  send_sms: sendSms,
  list_messages: listMessages,
  screenshot: screenshot,
  tap: tap,
  type: typeText,
  press_key: pressKey,
};

export const executors: ProviderExecutors = defineProviderExecutors<AndroidBridgeContext>({
  service: "android_bridge",
  handlers: actionHandlers,
  async createContext(context: ExecutionContext, fetcher: ProviderFetch): Promise<AndroidBridgeContext> {
    const credential = await requireCustomCredential(context, "android_bridge");
    return { values: credential.values, fetcher };
  },
});

export const credentialValidators: CredentialValidators = {
  customCredential: validateAndroidBridgeCredential,
};
