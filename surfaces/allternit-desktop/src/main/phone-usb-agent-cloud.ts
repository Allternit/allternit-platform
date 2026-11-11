/**
 * Cloud client for Link board relay registration (hardware/link/README.md
 * sections 4.2 and 4.5). cloud-api keeps one `link_boards` row per account
 * and board; registering returns the board's relay token once, which Desktop
 * writes to the board over USB serial (`prov set rtok`) and never keeps.
 *
 *   POST   /api/v1/link-boards              { boardId, name, runtimeDeviceId } → { boardId, relayToken, runtimeDeviceId }
 *   GET    /api/v1/link-boards              → { boards: [{ boardId, …, revokedAt }] }
 *   DELETE /api/v1/link-boards/:boardId     → { revoked: true }
 *
 * Rules for this file: the relay token is returned to the caller only; it is
 * never logged, never part of an error message, never written to disk.
 */

export interface LinkBoardCloud {
  register(boardId: string, name: string): Promise<{ relayToken: string; relayHost: string }>;
  revoke(boardId: string): Promise<void>;
  list(): Promise<Array<{ boardId: string; revokedAt: string | null }>>;
}

export type LinkBoardCloudErrorCode = 'signed_out' | 'http' | 'network' | 'bad_answer';

export class LinkBoardCloudError extends Error {
  constructor(
    readonly code: LinkBoardCloudErrorCode,
    message: string,
    readonly status?: number,
  ) {
    super(message);
    this.name = 'LinkBoardCloudError';
  }
}

const BOARD_ID = /^usba-[0-9a-f]{12}$/;
const RELAY_TOKEN = /^[0-9a-f]{64}$/;
/** What the firmware accepts for `prov set relay` (contract section 2.9). */
const RELAY_HOST = /^[a-z0-9.-]{1,63}$/;
const TIMEOUT_MS = 15_000;

/** The relay host a board dials: the cloud-api host this Desktop talks to. */
export function relayHostOf(cloudUrl: string): string {
  let host: string;
  try {
    host = new URL(cloudUrl).hostname.toLowerCase();
  } catch {
    throw new LinkBoardCloudError('bad_answer', 'The cloud address is not a valid URL.');
  }
  if (!RELAY_HOST.test(host)) throw new LinkBoardCloudError('bad_answer', `The cloud host ${host.slice(0, 80)} cannot be stored on a board.`);
  return host;
}

export function httpLinkBoardCloud(deps: {
  cloudUrl: () => string;
  headers: () => Record<string, string>;
  runtimeDeviceId: () => string | null;
  fetchImpl?: typeof fetch;
}): LinkBoardCloud {
  const doFetch = deps.fetchImpl ?? fetch;
  const base = () => deps.cloudUrl().replace(/\/+$/, '');

  const credentials = (): Record<string, string> => {
    const headers = deps.headers();
    if (!Object.keys(headers).some((k) => k.toLowerCase() === 'authorization')) {
      throw new LinkBoardCloudError('signed_out', 'Sign in to Allternit to reach the board through the cloud.');
    }
    return headers;
  };

  const request = async (method: string, path: string, body?: unknown): Promise<unknown> => {
    const headers = { ...credentials(), ...(body === undefined ? {} : { 'content-type': 'application/json' }) };
    let res: Response;
    try {
      res = await doFetch(`${base()}${path}`, {
        method,
        headers,
        body: body === undefined ? undefined : JSON.stringify(body),
        signal: AbortSignal.timeout(TIMEOUT_MS),
      });
    } catch (error) {
      throw new LinkBoardCloudError('network', `Could not reach the cloud: ${error instanceof Error ? error.message : String(error)}`);
    }
    const parsed = (await res.json().catch(() => null)) as unknown;
    if (!res.ok) {
      const b = (parsed ?? {}) as { message?: unknown; error?: unknown };
      const detail = typeof b.message === 'string' ? b.message : typeof b.error === 'string' ? b.error : `HTTP ${res.status}`;
      throw new LinkBoardCloudError(res.status === 401 ? 'signed_out' : 'http', `The cloud answered ${res.status}: ${detail.slice(0, 200)}`, res.status);
    }
    return parsed;
  };

  const boardPath = (boardId: string) => {
    if (!BOARD_ID.test(boardId)) throw new LinkBoardCloudError('bad_answer', 'Not a board id.');
    return `/api/v1/link-boards/${boardId}`;
  };

  return {
    async register(boardId, name) {
      boardPath(boardId);
      const runtimeDeviceId = deps.runtimeDeviceId();
      if (!runtimeDeviceId) throw new LinkBoardCloudError('signed_out', 'Sign in to Allternit to reach the board through the cloud.');
      const relayHost = relayHostOf(deps.cloudUrl());
      const answer = (await request('POST', '/api/v1/link-boards', { boardId, name: name.trim(), runtimeDeviceId })) as { boardId?: unknown; relayToken?: unknown } | null;
      if (!answer || answer.boardId !== boardId || typeof answer.relayToken !== 'string' || !RELAY_TOKEN.test(answer.relayToken)) {
        throw new LinkBoardCloudError('bad_answer', 'The cloud did not return a relay token for this board.');
      }
      return { relayToken: answer.relayToken, relayHost };
    },

    async revoke(boardId) {
      try {
        await request('DELETE', boardPath(boardId));
      } catch (error) {
        // 404: this account has no row for the board, so no token of it is live.
        if (error instanceof LinkBoardCloudError && error.status === 404) return;
        throw error;
      }
    },

    async list() {
      const answer = (await request('GET', '/api/v1/link-boards')) as { boards?: unknown } | null;
      if (!answer || !Array.isArray(answer.boards)) throw new LinkBoardCloudError('bad_answer', 'The cloud board list was not understood.');
      return answer.boards.flatMap((row) => {
        const r = row as { boardId?: unknown; revokedAt?: unknown } | null;
        if (!r || typeof r.boardId !== 'string') return [];
        return [{ boardId: r.boardId, revokedAt: typeof r.revokedAt === 'string' ? r.revokedAt : null }];
      });
    },
  };
}
