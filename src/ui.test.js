import { expect, test } from 'bun:test';
import { cycleKeyed, filterChoices, keyedValues, normalizeKeyed, renderLabel, selectableIndex, workingDirectoryLine } from './ui.js';

const plain = (value) => value.replace(/\x1b\[[0-9;]*m/g, '');

const choices = [
  { label: '\x1b[1mClaude Sonnet 5\x1b[0m', value: 'anthropic/claude-sonnet-5' },
  { label: 'Gemini Pro', value: 'google/gemini-3-pro' },
  { label: 'Llama', value: 'meta-llama/llama-4' }
];

test('choice filtering matches every typed term against labels', () => {
  expect(filterChoices(choices, 'claude sonnet')).toEqual([choices[0]]);
});

test('choice filtering matches provider model ids and ignores case', () => {
  expect(filterChoices(choices, 'GOOGLE/GEMINI')).toEqual([choices[1]]);
});

test('an empty filter preserves the full list', () => {
  expect(filterChoices(choices, '  ')).toBe(choices);
});

test('rows with a non-string value are searched through their filterText', () => {
  const sessions = [
    { label: '2h  fix the pool proxy', value: { kind: 'session', id: 'a1' }, filterText: 'a1 J:\\justgains main' },
    { label: '3d  rewrite the picker', value: { kind: 'session', id: 'b2' }, filterText: 'b2 F:\\bro-cli main' }
  ];
  expect(filterChoices(sessions, 'justgains')).toEqual([sessions[0]]);
  expect(filterChoices(sessions, 'b2')).toEqual([sessions[1]]);
});

test('a search drops group dividers instead of leaving rules over nothing', () => {
  const rows = [{ divider: true, label: 'this project' }, ...choices];
  expect(filterChoices(rows, 'claude')).toEqual([choices[0]]);
  expect(filterChoices(rows, '')).toBe(rows);
});

test('a column header stays above the matches and disappears with them', () => {
  const header = { divider: true, header: true, label: 'model  age  cost' };
  const rows = [header, { divider: true, label: 'group' }, ...choices];
  expect(filterChoices(rows, 'llama')).toEqual([header, choices[2]]);
  expect(filterChoices(rows, 'nothing-here')).toEqual([]);
});

test('width-dependent labels are searched through their filterText and rendered for the width', () => {
  const row = { label: (width) => `Sonnet${' '.repeat(width - 6)}★★★★★`, value: 'anthropic/claude-sonnet-5', filterText: 'Anthropic: Claude Sonnet 5' };
  expect(filterChoices([row], 'anthropic sonnet')).toEqual([row]);
  expect(filterChoices([row], 'zzz')).toEqual([]);
  expect(renderLabel(row, 20)).toBe('Sonnet              ★★★★★');
  expect(renderLabel(choices[1], 20)).toBe('Gemini Pro');
  expect(renderLabel({ divider: true, label: 'x' }, 20)).toBe('');
});

test('a keyed toggle with options rotates through them and wraps', () => {
  const harness = { key: 'h', name: 'harness', value: 'omp', options: ['claude', 'omp', 'pi', 'codex'] };
  const [keyed] = normalizeKeyed([harness]);

  expect(keyedValues([keyed])).toEqual({ harness: 'omp' });
  cycleKeyed(keyed);
  expect(keyedValues([keyed])).toEqual({ harness: 'pi' });
  cycleKeyed(keyed);
  expect(keyedValues([keyed])).toEqual({ harness: 'codex' });
  cycleKeyed(keyed);
  expect(keyedValues([keyed])).toEqual({ harness: 'claude' });
});

test('an unknown starting value opens the rotation on its first option', () => {
  const [keyed] = normalizeKeyed([{ key: 'h', name: 'harness', value: 'gone', options: ['claude', 'omp'] }]);
  expect(keyedValues([keyed])).toEqual({ harness: 'claude' });
});

test('a keyed toggle without options stays a plain on/off switch', () => {
  const [keyed] = normalizeKeyed([{ key: 's', name: 'skip', value: true }]);
  cycleKeyed(keyed);
  expect(keyedValues([keyed])).toEqual({ skip: false });
});

test('the cursor lands past a divider, from either direction', () => {
  const rows = [{ divider: true }, { value: 'a' }, { value: 'b' }, { divider: true }];
  expect(selectableIndex(rows, 0)).toBe(1);
  expect(selectableIndex(rows, 3, -1)).toBe(2);
  // A list of nothing but dividers has no landing spot to report.
  expect(selectableIndex([{ divider: true }], 0)).toBe(-1);
});

test('the picker shows its working directory with the active folder emphasized', () => {
  const line = workingDirectoryLine('F:\\bro-cli', 80);
  expect(plain(line)).toBe('  ◆  working directory  F:\\bro-cli');
  expect(line).toContain('\x1b[2mworking directory  F:\\');
  expect(line).toContain('\x1b[1;96mbro-cli\x1b[0m');
});

test('a long working directory keeps its useful tail within the terminal width', () => {
  const line = workingDirectoryLine('J:\\very\\deeply\\nested\\workspace\\project-name', 48);
  expect(plain(line).length).toBeLessThanOrEqual(48);
  expect(plain(line)).toContain('…');
  expect(plain(line)).toEndWith('project-name');
});
