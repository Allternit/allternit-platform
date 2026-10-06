import { describe, expect, it } from 'vitest';
import {
  buildBotComputerWindowUrl,
  isBotComputerWindowUrl,
} from './bot-computer-window.js';

describe('buildBotComputerWindowUrl', () => {
  it('builds a lightweight /bot-computer URL', () => {
    const href = buildBotComputerWindowUrl('https://platform.example', {
      botId: 'bot-1',
      title: "Gizzi's computer",
    });
    const url = new URL(href);
    expect(url.pathname).toBe('/bot-computer');
    expect(url.searchParams.get('botId')).toBe('bot-1');
    expect(url.searchParams.get('title')).toBe("Gizzi's computer");
  });

  it('forwards sandboxId onto the detached URL', () => {
    const href = buildBotComputerWindowUrl('https://platform.example', {
      botId: 'bot-1',
      sandboxId: 'account-box',
    });
    expect(new URL(href).searchParams.get('sandboxId')).toBe('account-box');
  });

  it('rejects a missing bot id', () => {
    expect(() => buildBotComputerWindowUrl('https://platform.example', { botId: '' })).toThrow(
      /bot ID/i,
    );
  });
});

describe('isBotComputerWindowUrl', () => {
  it('matches /bot-computer and legacy /shell detached computer URLs', () => {
    const lite = new URL(buildBotComputerWindowUrl('https://platform.example', { botId: 'b' }));
    expect(isBotComputerWindowUrl(lite)).toBe(true);
    const legacy = new URL('https://platform.example/shell?detachedSurface=bot-computer&botId=b');
    expect(isBotComputerWindowUrl(legacy)).toBe(true);
  });

  it('rejects other detached surfaces', () => {
    const code = new URL('https://platform.example/shell?detachedSurface=code&detachedSessionId=s');
    expect(isBotComputerWindowUrl(code)).toBe(false);
  });
});

describe('bot phone window', () => {
  it('builds and recognises the /bot-phone URL', async () => {
    const { buildBotPhoneWindowUrl, isBotPhoneWindowUrl } = await import('./bot-computer-window.js');
    const url = new URL(buildBotPhoneWindowUrl('https://platform.example', 'bot 1'));
    expect([url.pathname, url.searchParams.get('botId')]).toEqual(['/bot-phone', 'bot 1']);
    expect(isBotPhoneWindowUrl(url)).toBe(true);
    expect(isBotPhoneWindowUrl(new URL('https://platform.example/bot-phone'))).toBe(false);
    expect(() => buildBotPhoneWindowUrl('https://platform.example', '')).toThrow();
  });
});
