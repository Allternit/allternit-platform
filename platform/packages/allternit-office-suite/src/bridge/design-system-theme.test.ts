import { describe, expect, it } from 'vitest';
import { designSystemTheme } from './design-system-theme';

describe('designSystemTheme', () => {
  it('maps a palette without hints: lightest neutral = page, darkest = text, chromatic = accents in order', () => {
    const t = designSystemTheme({
      name: 'Talvik',
      colors: [
        { name: 'talv-snow', value: '#F2F4F1' },
        { name: 'talv-stone', value: '#9FA79E' },
        { name: 'talv-granite', value: '#7E877D' },
        { name: 'talv-shale', value: '#3E4A43' },
        { name: 'talv-spruce', value: '#141B18' },
        { name: 'talv-glacier', value: '#2FC4CF' },
        { name: 'talv-ember', value: '#E86A2B' },
      ],
      fonts: ['Bricolage Grotesque', 'Barlow'],
    })!;
    expect(t.colors.lt1).toBe('F2F4F1');
    expect(t.colors.dk1).toBe('141B18');
    expect(t.colors.dk2).toBe('3E4A43');
    expect(t.colors.accent1).toBe('2FC4CF');
    expect(t.colors.accent2).toBe('E86A2B');
    expect(t.colors.hlink).toBe('2FC4CF');
    expect(Object.values(t.colors).every((v) => /^[0-9A-F]{6}$/.test(v))).toBe(true);
    expect(t.majorFont).toBe('Bricolage Grotesque');
    expect(t.minorFont).toBe('Barlow');
  });

  it('follows names: background, text and primary', () => {
    const t = designSystemTheme({
      name: 'Agentic',
      colors: [
        { name: 'Danger', value: '#DC2626' },
        { name: 'Primary', value: '#FF5701' },
        { name: 'Surface', value: '#FFFFFF' },
        { name: 'Text', value: '#111827' },
        { name: 'Secondary', value: '#F6F6F1' },
      ],
      fonts: [],
    })!;
    expect(t.colors.lt1).toBe('FFFFFF');
    expect(t.colors.dk1).toBe('111827');
    expect(t.colors.accent1).toBe('FF5701');
    expect(t.majorFont).toBeUndefined();
  });

  it('falls back to lightest/darkest when the named pair has too little contrast', () => {
    const t = designSystemTheme({
      name: 'Low',
      colors: [
        { name: 'background', value: '#BBBBBB' },
        { name: 'text', value: '#555555' },
        { name: 'white', value: '#FFFFFF' },
        { name: 'black', value: '#000000' },
      ],
      fonts: [],
    })!;
    expect([t.colors.lt1, t.colors.dk1]).toEqual(['FFFFFF', '000000']);
  });

  it('returns null with fewer than two colours', () => {
    expect(designSystemTheme({ name: 'x', colors: [{ name: 'a', value: '#123456' }], fonts: [] })).toBeNull();
  });
});
