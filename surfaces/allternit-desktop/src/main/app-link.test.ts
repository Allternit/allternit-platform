import { describe, expect, it } from 'vitest';
import { appShellUrl, appSlugFromLink, isAppLink } from './app-link.js';

describe('allternit://app/<slug>', () => {
  it('takes the slug and nothing else', () => {
    expect(isAppLink('allternit://app/notes')).toBe(true);
    expect(appSlugFromLink('allternit://app/notes')).toBe('notes');
    expect(appSlugFromLink('allternit-dev://app/notes?handoff=abc')).toBe('notes');
  });

  it('rejects other hosts, extra path parts and bad slugs', () => {
    expect(isAppLink('allternit://apps/notes')).toBe(false);
    expect(isAppLink('allternit://pair?code=1')).toBe(false);
    expect(appSlugFromLink('allternit://app/notes/../x')).toBeNull();
    expect(appSlugFromLink('allternit://app/notes/extra')).toBeNull();
    expect(appSlugFromLink('allternit://app/9lives')).toBeNull();
    expect(appSlugFromLink('allternit://app/')).toBeNull();
    expect(appSlugFromLink('https://app/notes')).toBeNull();
  });

  it('builds the app shell route on the Desktop UI origin', () => {
    expect(appShellUrl('https://ai.allternit.com/platform?x=1', 'notes')).toBe('https://ai.allternit.com/app/notes');
    expect(() => appShellUrl('https://ai.allternit.com', 'Bad')).toThrow();
  });
});
