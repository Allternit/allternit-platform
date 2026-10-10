import { describe, expect, it, vi } from 'vitest';
import { BrowserEventSchema, COMPUTER_USE_PROTOCOL_VERSION } from '@allternit/computer-use-protocol';
import { ExtensionTabProvider, RemoteBrowserProvider, createStagehandProvider } from './remote-provider.js';

describe('remote browser providers', () => {
  it('validates remote provider events from the remote endpoint', async () => {
    const fetchImpl = vi.fn(async () => new Response(JSON.stringify([
      BrowserEventSchema.parse({
        schemaVersion: COMPUTER_USE_PROTOCOL_VERSION,
        eventId: 'event-1',
        runId: 'run-1',
        sessionId: 'session-1',
        sequence: 1,
        emittedAt: '2026-07-10T20:00:00.000Z',
        type: 'action.state_changed',
        payload: { actionId: 'action-1', state: 'committed' },
      }),
    ]), { status: 200 }));
    const provider = new RemoteBrowserProvider({
      baseUrl: 'https://browser.internal',
      fetchImpl,
      capabilities: {
        provider: 'local-playwright',
        capabilities: ['navigate'],
        local: false,
        attachedToUserSession: false,
        supportsPrivateNetwork: false,
        supportsPersistentProfile: false,
        limits: {},
      },
    });
    const events = await provider.execute({
      schemaVersion: COMPUTER_USE_PROTOCOL_VERSION,
      actionId: 'action-1',
      runId: 'run-1',
      sessionId: 'session-1',
      kind: 'navigate',
      reason: 'Open page',
      input: { url: 'https://example.com/' },
    });
    expect(provider.capabilities.provider).toBe('local-playwright');
    expect(events[0].type).toBe('action.state_changed');
    expect(fetchImpl).toHaveBeenCalledWith(new URL('/v1/browser/actions', 'https://browser.internal'), expect.objectContaining({
      method: 'POST',
    }));
  });

  it('exposes Stagehand as the same provider interface', () => {
    const provider = createStagehandProvider({ baseUrl: 'https://stagehand.internal' });
    expect(provider.capabilities.provider).toBe('stagehand');
    expect(provider.capabilities.capabilities).toContain('network.inspect');
  });

  it('bridges extension current-tab commands without a parallel runtime contract', async () => {
    const send = vi.fn(async () => ([
      BrowserEventSchema.parse({
        schemaVersion: COMPUTER_USE_PROTOCOL_VERSION,
        eventId: 'event-extension',
        runId: 'run-extension',
        sessionId: 'session-extension',
        sequence: 1,
        emittedAt: '2026-07-10T20:00:00.000Z',
        type: 'action.state_changed',
        payload: { actionId: 'action-extension', state: 'committed' },
      }),
    ]));
    const provider = new ExtensionTabProvider({ send });
    const events = await provider.execute({
      schemaVersion: COMPUTER_USE_PROTOCOL_VERSION,
      actionId: 'action-extension',
      runId: 'run-extension',
      sessionId: 'session-extension',
      kind: 'click',
      reason: 'Click in the active tab',
      targetRef: 'e1',
      input: {},
    });
    expect(provider.capabilities.provider).toBe('extension-tab');
    expect(events).toHaveLength(1);
    expect(send).toHaveBeenCalledWith(expect.objectContaining({ type: 'browser.execute' }));
  });
});
