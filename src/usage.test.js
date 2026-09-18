import { afterEach, expect, test } from 'bun:test';
import {
  appHeadroom,
  claudeHeadroom,
  clearUsageCache,
  codexHeadroom,
  codexMeters,
  leftLines,
  metersText,
  peekUsage,
  repaintOnUsage,
  requestUsage
} from './usage.js';
import { ICONS, iconNames } from './icons.js';

const plain = (value) => value.replace(/\x1b\[[0-9;]*m/g, '');
const named = (value) => plain(iconNames(value));

afterEach(() => clearUsageCache());

test('Claude headroom takes the tighter of the overall and Fable limits', () => {
  // A Max account: plenty of weekly left, almost no Fable.
  expect(claudeHeadroom({ session: 20, weekly: 71, fable: 98 })).toEqual({ h5: 80, wk: 29, fable5h: 80, fableWk: 2 });
  expect(claudeHeadroom(null)).toBeNull();
});

test('an account whose usage reports no Fable limit has no Fable access, not its overall headroom', () => {
  expect(claudeHeadroom({ session: 0, weekly: 41, fable: null })).toEqual({ h5: 100, wk: 59, fable5h: null, fableWk: null });
});

test('an exhausted weekly allowance leaves nothing in the 5h window either', () => {
  expect(claudeHeadroom({ session: 10, weekly: 100, fable: null })).toEqual({ h5: 0, wk: 0, fable5h: null, fableWk: null });
  expect(claudeHeadroom({ session: 10, weekly: 100, fable: 20 })).toEqual({ h5: 0, wk: 0, fable5h: 0, fableWk: 0 });
  // Fable alone exhausted: the account still has general 5h headroom, but none for Fable.
  expect(claudeHeadroom({ session: 10, weekly: 50, fable: 100 })).toEqual({ h5: 90, wk: 50, fable5h: 0, fableWk: 0 });
  expect(codexHeadroom({ primary: { usedPercent: 5, windowDurationMins: 300 }, secondary: { usedPercent: 100, windowDurationMins: 10_080 } }))
    .toEqual({ h5: 0, wk: 0 });
});

test('Codex windows are placed by their length, not their position', () => {
  // Pro reports only the weekly window, as its primary.
  expect(codexMeters({ primary: { usedPercent: 94, windowDurationMins: 10_080 }, secondary: null }))
    .toEqual({ session: null, weekly: 94 });
  expect(codexMeters({
    primary: { usedPercent: 42, windowDurationMins: 300 },
    secondary: { usedPercent: 7.5, windowDurationMins: 10_080 }
  })).toEqual({ session: 42, weekly: 7.5 });
  // Without lengths the historical order holds.
  expect(codexMeters({ primary: { usedPercent: 1 }, secondary: { usedPercent: 2 } })).toEqual({ session: 1, weekly: 2 });
  expect(codexHeadroom({ primary: { usedPercent: 94, windowDurationMins: 10_080 } })).toEqual({ h5: null, wk: 6 });
});

const weekly = (usedPercent) => ({ primary: { usedPercent, windowDurationMins: 10_080 }, secondary: null });

// Four Claude accounts and two Codex logins, as a real pool looks.
const POOL = {
  claude: [
    { stats: { session: 1, weekly: 71, fable: 98 } },
    { stats: { session: 0, weekly: 41, fable: null } },
    { stats: { session: 0, weekly: 19, fable: null } },
    { stats: { session: 0, weekly: 48, fable: null } }
  ],
  codex: [{ stats: weekly(94) }, { stats: weekly(74) }]
};

test('each app has its own 100% per window, averaged over its accounts', () => {
  const apps = appHeadroom(POOL);
  // Claude: 99 + 100 + 100 + 100 of its 5h, 29 + 59 + 81 + 52 of its week.
  expect(apps.claude).toMatchObject({ h5: 100, wk: 55, logins: 4, loading: false, unavailable: 0 });
  // Codex: no plan here has a 5h window; 6 + 26 of its week.
  expect(apps.codex).toMatchObject({ h5: null, wk: 16, logins: 2 });
  // Fresh accounts are 100%, never more.
  const fresh = appHeadroom({ claude: [{ stats: { session: 0, weekly: 0, fable: 0 } }, { stats: { session: 0, weekly: 0, fable: 0 } }] });
  expect(fresh.claude).toMatchObject({ h5: 100, wk: 100, fable5h: 100, fableWk: 100 });
});

test('Fable is shared only by the accounts that have it', () => {
  // The one account with Fable: 2% of its Fable week left, its 5h still open for Fable.
  expect(appHeadroom(POOL).claude).toMatchObject({ fable5h: 99, fableWk: 2 });
  const apps = appHeadroom({
    claude: [
      { stats: { session: 0, weekly: 50, fable: 90 } },
      { stats: { session: 0, weekly: 20, fable: 30 } },
      { stats: { session: 0, weekly: 0, fable: null } }
    ]
  });
  // (10 + 70) of 200 — the untouched account without Fable adds nothing.
  expect(apps.claude.fableWk).toBe(40);
  // No account with Fable at all: no Fable figure, not 0%.
  expect(appHeadroom({ claude: [{ stats: { session: 0, weekly: 0, fable: null } }] }).claude.fableWk).toBeNull();
});

test('a login signed in under two names counts once, and failed ones stay out', () => {
  const { codex } = appHeadroom({
    codex: [
      { stats: weekly(94), identity: 'user-a' },
      { stats: weekly(94), identity: 'user-a' },
      { stats: weekly(74), identity: 'user-b' },
      { stats: null, identity: 'user-c' }
    ]
  });
  expect(codex).toMatchObject({ wk: 16, unavailable: 1 });
});

test('the summary is a heading, a Claude line with Fable, and a Codex line', () => {
  const lines = leftLines(appHeadroom({ ...POOL, codex: [...POOL.codex, { stats: null }] })).map((line) => plain(line()));
  expect(lines).toEqual([
    '       5h   week              5h   week',
    `${ICONS.claude}   100%    55%    Fable    99%     2%`,
    `${ICONS.codex}      —    16%    1 unavailable`
  ]);
  // Every figure ends in the column its heading ends in (an icon is two cells).
  const endColumn = (text, needle) => [...text.slice(0, text.indexOf(needle) + needle.length)]
    .reduce((width, char) => width + (char.codePointAt(0) >= 0x10ff00 ? 2 : 1), 0);
  expect(endColumn(lines[1], '100%')).toBe(endColumn(lines[0], '5h'));
  expect(endColumn(lines[1], '55%')).toBe(endColumn(lines[0], 'week'));
  expect(endColumn(lines[2], '16%')).toBe(endColumn(lines[0], 'week'));
  expect(endColumn(lines[1], '99%')).toBe(lines[0].lastIndexOf('5h') + 2);
  expect(endColumn(lines[1], '2%')).toBe(lines[0].length);
});

test('an app shows placeholders while its logins load, and no line without any', () => {
  const loading = leftLines(appHeadroom({ claude: POOL.claude, codex: [{ stats: undefined }] })).map((line) => plain(line()));
  expect(loading[2]).toBe(`${ICONS.codex}      …      …`);
  // Claude's line doesn't wait for Codex.
  expect(loading[1]).toContain('55%');
  const claudeOnly = leftLines(appHeadroom({ claude: POOL.claude })).map((line) => plain(line()));
  expect(claudeOnly).toHaveLength(2);
  expect(leftLines(appHeadroom({}))).toEqual([]);
  // Codex alone has no Fable columns.
  expect(plain(leftLines(appHeadroom({ codex: POOL.codex }))[0]())).toBe('       5h   week');
});

test('pending meters keep the row shape with placeholders', () => {
  expect(plain(metersText([['5h', 17], ['wk', null]]))).toBe('5h 17% · wk —');
  expect(plain(metersText([['5h'], ['wk']], { pending: true }))).toBe('5h … · wk …');
});

test('a login is fetched once, shared while fresh, and a failure resolves null', async () => {
  let calls = 0;
  const fetcher = async () => ({ n: ++calls });
  expect(peekUsage('a')).toBeUndefined();
  const first = requestUsage('a', fetcher);
  const second = requestUsage('a', fetcher);
  expect(second).toBe(first);
  expect(peekUsage('a')).toBeUndefined();
  expect(await first).toEqual({ n: 1 });
  expect(peekUsage('a')).toEqual({ n: 1 });
  expect(await requestUsage('a', fetcher)).toEqual({ n: 1 });
  // Past its age a login is fetched again.
  expect(await requestUsage('a', fetcher, { maxAge: 0 })).toEqual({ n: 2 });

  expect(await requestUsage('b', async () => { throw new Error('401'); })).toBeNull();
  expect(peekUsage('b')).toBeNull();
});

test('a live menu repaints as usage lands and stops once closed', async () => {
  let release;
  const slow = new Promise((resolve) => { release = resolve; });
  const fast = Promise.resolve();
  let repaints = 0;
  const stop = repaintOnUsage([fast, slow])(() => { repaints++; });
  await fast;
  await Promise.resolve();
  expect(repaints).toBe(1);
  stop();
  release();
  await slow;
  await Promise.resolve();
  expect(repaints).toBe(1);
});
