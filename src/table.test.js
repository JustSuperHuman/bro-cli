import { expect, test } from 'bun:test';
import { bar, table } from './table.js';

test('columns size to their widest cell and numeric ones right-align', () => {
  expect(table({
    headers: ['Profile', 'Tokens'],
    rows: [['local', '1,000'], ['a-much-longer-name', '7']],
    numeric: [1]
  })).toBe([
    '┌────────────────────┬────────┐',
    '│ Profile            │ Tokens │',
    '├────────────────────┼────────┤',
    '│ local              │  1,000 │',
    '│ a-much-longer-name │      7 │',
    '└────────────────────┴────────┘'
  ].join('\n'));
});

test('rules draw a divider above the listed rows', () => {
  const rendered = table({ headers: ['A'], rows: [['one'], ['two'], ['sum']], rules: [2] });
  expect(rendered.split('\n')).toEqual([
    '┌─────┐',
    '│ A   │',
    '├─────┤',
    '│ one │',
    '│ two │',
    '├─────┤',
    '│ sum │',
    '└─────┘'
  ]);
});

test('widths ignore ANSI styling so tinted cells still line up', () => {
  const rendered = table({ headers: ['A'], rows: [['\x1b[2mdim\x1b[0m'], ['wider']] });
  expect(rendered).toContain('│ \x1b[2mdim\x1b[0m   │');
  expect(rendered).toContain('│ wider │');
});

test('bars keep a visible sliver for any non-zero share and fill exactly at 100%', () => {
  expect(bar(0, 10)).toBe('░░░░░░░░░░');
  expect(bar(1, 10)).toBe('██████████');
  expect(bar(0.5, 10)).toBe('█████░░░░░');
  expect(bar(0.75, 10)).toBe('███████▌░░');
  expect(bar(0.0001, 10)).toBe('▏░░░░░░░░░');
  expect([...bar(0.37, 10)]).toHaveLength(10);
});

test('bars clamp out-of-range and non-numeric shares instead of overflowing the cell', () => {
  expect(bar(2, 6)).toBe('██████');
  expect(bar(-1, 6)).toBe('░░░░░░');
  expect(bar(Number.NaN, 6)).toBe('░░░░░░');
});
