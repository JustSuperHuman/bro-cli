import { expect, test } from 'bun:test';
import { filterChoices, selectableIndex } from './ui.js';

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

test('the cursor lands past a divider, from either direction', () => {
  const rows = [{ divider: true }, { value: 'a' }, { value: 'b' }, { divider: true }];
  expect(selectableIndex(rows, 0)).toBe(1);
  expect(selectableIndex(rows, 3, -1)).toBe(2);
  // A list of nothing but dividers has no landing spot to report.
  expect(selectableIndex([{ divider: true }], 0)).toBe(-1);
});
