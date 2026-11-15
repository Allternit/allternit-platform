import { describe, expect, it } from 'vitest';
import { isModelLink, modelLinkQuery, modelLinkTarget } from './model-link.js';

describe('model deep links', () => {
  it('maps a chat link and a Studio link to the web params', () => {
    expect(modelLinkQuery('allternit://open?model=glm-5.3-flash')?.toString()).toBe('model=glm-5.3-flash');
    expect(modelLinkQuery('allternit://open?view=model-lab&model=Qwen/Qwen3-8B')?.toString()).toBe(
      'view=model-lab&model=Qwen%2FQwen3-8B',
    );
    expect(modelLinkQuery('allternit-dev://open?model=x1')?.toString()).toBe('model=x1');
  });
  it('builds the app URL from the platform URL only', () => {
    expect(modelLinkTarget('https://ai.allternit.com/', 'allternit://open?model=a-b')).toBe(
      'https://ai.allternit.com/?model=a-b',
    );
    expect(modelLinkTarget('https://ai.allternit.com/?view=chat', 'allternit://open?view=model-lab&model=o/n')).toBe(
      'https://ai.allternit.com/?view=model-lab&model=o%2Fn',
    );
  });
  it('rejects unsafe ids, other views, URLs and other hosts', () => {
    for (const bad of [
      'allternit://open?model=',
      'allternit://open?model=https://evil.example/x',
      'allternit://open?model=../../etc',
      'allternit://open?model=a%20b',
      'allternit://open?model=a/b/c',
      'allternit://open?model=%3Cscript%3E',
      'allternit://open?model=javascript:alert(1)',
      `allternit://open?model=${'a'.repeat(201)}`,
      'allternit://open?view=settings&model=x',
      'allternit://open?view=&model=x',
      'allternit://other?model=x',
      'https://open/?model=x',
      'not a url',
    ]) {
      expect(modelLinkQuery(bad)).toBeNull();
    }
  });
  it('drops extra params', () => {
    expect(modelLinkQuery('allternit://open?model=x&url=https://evil.example&q=hi')?.toString()).toBe('model=x');
  });
  it('recognizes only open links that carry model', () => {
    expect(isModelLink('allternit://open?model=x')).toBe(true);
    expect(isModelLink('allternit://open/hud')).toBe(false);
    expect(isModelLink('allternit://design?prompt=x')).toBe(false);
  });
});
