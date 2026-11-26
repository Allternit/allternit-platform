import { describe, expect, it } from 'vitest';
import { artifactIdFromLink, artifactLinkTarget, isArtifactLink } from './artifact-link.js';

describe('allternit://artifact/<id>', () => {
  it('takes the id and nothing else', () => {
    expect(isArtifactLink('allternit://artifact/art_abc123')).toBe(true);
    expect(artifactIdFromLink('allternit://artifact/art_abc123')).toBe('art_abc123');
    expect(artifactIdFromLink('allternit-dev://artifact/art_A-b_9?x=1')).toBe('art_A-b_9');
  });

  it('rejects other hosts, extra parts, escapes and bad ids', () => {
    expect(isArtifactLink('allternit://artifacts/art_1')).toBe(false);
    expect(isArtifactLink('allternit://session?id=ses_1')).toBe(false);
    expect(artifactIdFromLink('allternit://artifact/')).toBeNull();
    expect(artifactIdFromLink('allternit://artifact/abc')).toBeNull();
    expect(artifactIdFromLink('allternit://artifact/art_')).toBeNull();
    expect(artifactIdFromLink('allternit://artifact/art_a/extra')).toBeNull();
    expect(artifactIdFromLink('allternit://artifact/art_a/../x')).toBeNull();
    expect(artifactIdFromLink('allternit://artifact/art_a%2Fb')).toBeNull();
    expect(artifactIdFromLink('allternit://artifact/art_a b')).toBeNull();
    expect(artifactIdFromLink(`allternit://artifact/art_${'a'.repeat(65)}`)).toBeNull();
    expect(artifactIdFromLink('https://artifact/art_a')).toBeNull();
  });

  it('builds /?artifact=<id> on the Desktop UI origin', () => {
    expect(artifactLinkTarget('https://ai.allternit.com/platform?x=1', 'allternit://artifact/art_abc')).toBe(
      'https://ai.allternit.com/?artifact=art_abc',
    );
    expect(artifactLinkTarget('https://ai.allternit.com', 'allternit://artifact/nope')).toBeNull();
  });
});
