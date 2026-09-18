import { expect, test } from 'bun:test';
import { brandBanner, titledBox } from './banner.js';
import { iconNames } from './icons.js';
import { visWidth } from './ui.js';

const plain = (value) => iconNames(value).replace(/\x1b\[[0-9;]*m/g, '');

// A Claude account and a Codex login signed in, their usage not in yet.
const loading = {
  accounts: [{ name: 'banner-test', authenticated: true }],
  logins: [{ name: 'banner-test', dir: '/nowhere/banner-test', authenticated: true }]
};

test('a titled box is square: every row the same width, the title in its top edge', () => {
  const box = titledBox('Usage', ['short', 'a longer line']);
  expect(box.map(plain)).toEqual([
    '╭─ Usage ───────╮',
    '│ short         │',
    '│ a longer line │',
    '╰───────────────╯'
  ]);
  expect(new Set(box.map(visWidth)).size).toBe(1);
});

test('the Usage section sits beside the logo when it fits, its rows lined up', () => {
  const rows = brandBanner(loading)(120).split('\n');
  // Logo and box share five rows, then a blank one before the picker.
  expect(rows).toHaveLength(6);
  expect(rows[5]).toBe('');
  const boxStart = plain(rows[0]).indexOf('╭─ Usage');
  expect(boxStart).toBeGreaterThan(40);
  expect(plain(rows[4]).indexOf('╰')).toBe(boxStart);
  expect(plain(rows[2])).toContain('│ Claude      …      …    Fable      …      … │');
  expect(plain(rows[3])).toContain('│ Codex      …      …');
});

test('on a narrow terminal the Usage section goes under the logo', () => {
  const rows = brandBanner(loading)(60).split('\n');
  expect(rows).toHaveLength(4 + 5 + 1);
  expect(plain(rows[4]).startsWith('╭─ Usage')).toBe(true);
});

test('with nothing signed in, or no usage asked for, it is the logo alone', () => {
  expect(brandBanner({ accounts: [], logins: [] })(120).split('\n')).toHaveLength(5);
  expect(brandBanner(null)(120)).not.toContain('Usage');
});
