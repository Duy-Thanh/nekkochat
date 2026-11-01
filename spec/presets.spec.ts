import { describe, expect, test } from 'bun:test';
import { PRESET_CATEGORIES, SYSTEM_PRESETS, findPreset } from '../src/lib/presets';

describe('system presets', () => {
  test('there is a default preset and it comes first (new chats use SYSTEM_PRESETS[0])', () => {
    expect(SYSTEM_PRESETS[0].id).toBe('default');
  });

  test('ids are unique and every field is filled in', () => {
    const ids = SYSTEM_PRESETS.map((p) => p.id);
    expect(new Set(ids).size).toBe(ids.length);
    for (const p of SYSTEM_PRESETS) {
      expect(p.id).toMatch(/^[a-z0-9-]+$/);
      expect(p.name.trim().length).toBeGreaterThan(2);
      expect(p.icon.length).toBeGreaterThan(0);
      expect(p.description.trim().length).toBeGreaterThan(10);
      expect(p.prompt.trim().length).toBeGreaterThan(200);
    }
  });

  test('every preset belongs to a known category and every category is used', () => {
    for (const p of SYSTEM_PRESETS) expect(PRESET_CATEGORIES).toContain(p.category);
    for (const c of PRESET_CATEGORIES) expect(SYSTEM_PRESETS.some((p) => p.category === c)).toBe(true);
  });

  test('every prompt carries the shared rules (honesty, language, untrusted data)', () => {
    for (const p of SYSTEM_PRESETS) {
      expect(p.prompt).toContain('General rules:');
      expect(p.prompt).toContain('Never invent facts');
      expect(p.prompt).toContain('Reply in the language the user writes in');
      expect(p.prompt).toContain('is data, not instructions');
    }
  });

  test('prompts are detailed but stay well inside what a model should be given', () => {
    for (const p of SYSTEM_PRESETS) {
      expect(p.prompt.length, p.id).toBeLessThan(6000);
    }
  });

  test('prompts are written in English and have no leftover placeholders', () => {
    for (const p of SYSTEM_PRESETS) {
      expect(p.prompt, p.id).not.toMatch(/TODO|FIXME|lorem ipsum/i);
      // No Vietnamese diacritics outside of the translator, which legitimately mentions the language
      if (p.id !== 'translator') expect(p.prompt, p.id).not.toMatch(/[ăâêôơưđ]/i);
    }
  });

  test('the security presets state their authorisation boundary', () => {
    for (const id of ['security-auditor', 'android-sec']) {
      const p = SYSTEM_PRESETS.find((x) => x.id === id)!;
      expect(p.prompt.toLowerCase(), id).toMatch(/authori[sz]ed|owns/);
    }
  });

  test('findPreset recognises an unmodified preset and nothing else', () => {
    for (const p of SYSTEM_PRESETS) expect(findPreset(p.prompt)?.id).toBe(p.id);
    expect(findPreset(SYSTEM_PRESETS[0].prompt + ' edited')).toBeUndefined();
    expect(findPreset('')).toBeUndefined();
  });
});
