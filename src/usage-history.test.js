import { afterEach, expect, test } from 'bun:test';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {
  measuredRatio,
  meterMovement,
  ratioFromMovement,
  recordReading,
  resetHistoryCache,
  windowId
} from './usage-history.js';

const temp = fs.mkdtempSync(path.join(os.tmpdir(), 'bro-usage-history-'));
let n = 0;
const freshFile = () => path.join(temp, `history-${++n}.json`);

afterEach(() => resetHistoryCache());

const HOUR = 60 * 60 * 1000;
const WEEK_RESET = '2026-09-23T17:00:00.043769+00:00';
const reading = (at, s, w, sr, wr = WEEK_RESET) => ({ at, s, w, sr: windowId(sr), wr: windowId(wr) });

test('a window is the same window whichever microsecond the API stamps it with', () => {
  expect(windowId('2026-09-20T14:59:59.226127+00:00')).toBe(windowId('2026-09-20T15:00:00.226282+00:00'));
  expect(windowId(null)).toBeNull();
});

test('the meters move together only within one 5-hour window of one week', () => {
  const window = '2026-09-18T17:10:00.1+00:00';
  const next = '2026-09-18T22:10:00.1+00:00';
  const readings = [
    { at: 0, s: 0, w: 19, sr: null, wr: windowId(WEEK_RESET) }, // no window open yet
    reading(1 * HOUR, 38, 24, window), // the window that opened: counts
    reading(2 * HOUR, 68, 29, window),
    reading(3 * HOUR, 100, 35, window),
    reading(6 * HOUR, 10, 36, next), // a new window: the drop doesn't count
    reading(7 * HOUR, 30, 39, next),
    reading(8 * HOUR, 40, 1, next, '2026-09-30T17:00:00Z') // a new week: doesn't count
  ];
  expect(meterMovement(readings)).toEqual({ session: 100 + 20, weekly: 16 + 3 });
  // Order of recording doesn't matter.
  expect(meterMovement([...readings].reverse())).toEqual({ session: 120, weekly: 19 });
});

test('there is no ratio until the 5-hour meter has moved enough to read it', () => {
  expect(ratioFromMovement({ session: 19, weekly: 3 })).toBeNull();
  expect(ratioFromMovement({ session: 100, weekly: 16 })).toEqual({ ratio: 0.16, sessionPoints: 100 });
});

test('readings are kept per account, repeats skipped, and an account borrows its tier\'s measure, then any plan\'s', () => {
  const file = freshFile();
  const window = '2026-09-18T17:10:00Z';
  const meters = (session, weekly) => ({ session, weekly, fable: null, sessionResetsAt: window, weeklyResetsAt: WEEK_RESET, tier: 'team_tier' });
  recordReading('team-a', { ...meters(0, 48), at: 1 }, { file });
  recordReading('team-a', { ...meters(0, 48), at: 2 }, { file }); // unchanged: not kept
  recordReading('team-a', { ...meters(55, 56), at: 3 }, { file });
  recordReading('team-a', { ...meters(65, 57), at: 4 }, { file });
  expect(JSON.parse(fs.readFileSync(file, 'utf8')).logins['team-a'].readings).toHaveLength(3);

  // 65 5h points against 9 weekly ones.
  const own = measuredRatio({ key: 'team-a', tier: 'team_tier' }, { file });
  expect(own.source).toBe('account');
  expect(own.ratio).toBeCloseTo(9 / 65);

  // A fresh account on the same tier uses the tier's measure; one on a tier
  // nobody has measured uses every plan's, and is told so.
  expect(measuredRatio({ key: 'team-b', tier: 'team_tier' }, { file })).toMatchObject({ source: 'tier', sessionPoints: 65 });
  expect(measuredRatio({ key: 'max-a', tier: 'max_tier' }, { file })).toMatchObject({ source: 'plans', sessionPoints: 65 });
  // With nothing measured anywhere, there's no ratio at all.
  expect(measuredRatio({ key: 'max-a', tier: 'max_tier' }, { file: freshFile() })).toBeNull();

  // The history survives a restart.
  resetHistoryCache();
  expect(measuredRatio({ key: 'team-a' }, { file }).ratio).toBeCloseTo(9 / 65);
});

test('readings without both meters, or without a key, are not kept', () => {
  const file = freshFile();
  recordReading('x', { session: null, weekly: 10, sessionResetsAt: null, weeklyResetsAt: WEEK_RESET }, { file });
  recordReading('', { session: 5, weekly: 10, sessionResetsAt: null, weeklyResetsAt: WEEK_RESET }, { file });
  expect(fs.existsSync(file)).toBe(false);
});
