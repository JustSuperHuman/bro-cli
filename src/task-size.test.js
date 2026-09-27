import { expect, test } from 'bun:test';
import {
  capacityOf,
  describeTaskChoice,
  isReady,
  pickBySize,
  READY_FLOOR,
  resolveTaskProfile,
  roomText
} from './task-size.js';
import { parseArgs } from './cli.js';

const login = (name, h5, wk, ratio = null) =>
  ({ name, label: name, ratio, room: h5 == null && wk == null ? null : { h5, wk } });

test('a large task takes the roomiest login, a small task the emptiest usable one', () => {
  const logins = [login('roomy', 90, 70), login('busy', 12, 40), login('spent', 1, 2)];

  expect(pickBySize(logins, 'large').name).toBe('roomy');
  // "spent" has less left than "busy", but its window is too far gone to
  // finish anything before it resets.
  expect(pickBySize(logins, 'small').name).toBe('busy');
});

test('both meters rank the logins, in weekly points', () => {
  // The week is the budget — it refills once a week, a 5-hour window five
  // times a day — so it ranks: the account two thirds through its week is
  // the emptier one, however wide open its current window happens to be.
  const logins = [login('week-heavy', 100, 25), login('week-light', 91, 57)];

  expect(pickBySize(logins, 'small').name).toBe('week-heavy');
  expect(pickBySize(logins, 'large').name).toBe('week-light');

  // The 5-hour window is the rate, so it breaks ties between equal weeks.
  const tied = [login('narrow-window', 20, 50), login('open-window', 80, 50)];
  expect(pickBySize(tied, 'large').name).toBe('open-window');
  expect(pickBySize(tied, 'small').name).toBe('narrow-window');
});

test('the 5-hour window is converted into weekly points before it is compared', () => {
  // A quarter of a weekly point per 5-hour point: a whole window open is
  // worth 25 weekly points, and the week caps it below that.
  expect(capacityOf({ h5: 100, wk: 40 }, { ratio: 0.25 })).toEqual({ week: 40, now: 25 });
  expect(capacityOf({ h5: 100, wk: 12 }, { ratio: 0.25 })).toEqual({ week: 12, now: 12 });
  // No measured ratio yet: the tighter of the two percentages still ranks
  // the logins the right way round, just more coarsely.
  expect(capacityOf({ h5: 30, wk: 80 })).toEqual({ week: 80, now: 30 });
  // A login reporting only one window is judged on the one it reports.
  expect(capacityOf({ h5: null, wk: 64 })).toEqual({ week: 64, now: 64 });
  expect(capacityOf({ h5: 42, wk: null })).toEqual({ week: 42, now: 42 });
  expect(capacityOf(null)).toBe(null);

  // Two accounts a week apart rank by the week, not by whose ratio is
  // kinder: converting only decides the tie and the floor.
  const logins = [login('slow-burn', 100, 30, 0.1), login('fast-burn', 100, 30, 0.5)];
  expect(pickBySize(logins, 'large').name).toBe('fast-burn');
  expect(pickBySize(logins, 'small').name).toBe('slow-burn');
});

test('a login spent in either window is passed over while another can finish', () => {
  // Wide-open week, but this 5-hour window is gone: nothing can start on it
  // for hours.
  expect(isReady({ h5: 2, wk: 90 })).toBe(false);
  // Wide-open window, but the week is gone: nothing can finish on it for
  // days.
  expect(isReady({ h5: 90, wk: 2 })).toBe(false);
  expect(isReady({ h5: 90, wk: 90 })).toBe(true);
  expect(isReady({ h5: null, wk: 90 })).toBe(true);
  expect(isReady(null)).toBe(false);

  const logins = [login('window-gone', 1, 80), login('usable', 60, 95)];
  expect(pickBySize(logins, 'small').name).toBe('usable');
  expect(pickBySize(logins, 'large').name).toBe('usable');
});

test('logins whose meters could not be read are passed over, not guessed at', () => {
  const logins = [login('unreadable', null, null), login('known', 25, 30)];

  expect(pickBySize(logins, 'large').name).toBe('known');
  expect(pickBySize(logins, 'small').name).toBe('known');
  expect(pickBySize([login('unreadable', null, null)], 'large')).toBe(null);
});

test('with every login spent a task still gets the best of a bad lot, and is told', async () => {
  const candidates = [login('a', 1, 4), login('b', 3, 40)];
  const chosen = await resolveTaskProfile({ app: 'claude', size: 'small', candidates });

  expect(chosen.name).toBe('a');
  expect(chosen.spent).toBe(true);
  expect(describeTaskChoice(chosen, 'small')).toContain('every login is spent somewhere');
  expect(READY_FLOOR).toBeGreaterThan(0);
});

test('the same pool ranks the same way twice running', () => {
  const logins = [login('b', 50, 50), login('a', 50, 50)];

  expect(pickBySize(logins, 'large').name).toBe('a');
  expect(pickBySize([...logins].reverse(), 'large').name).toBe('a');
});

test('resolving says what it chose, and admits when it cannot choose', async () => {
  const candidates = [login('big', 88, 60), login('little', 40, 20)];

  const large = await resolveTaskProfile({ app: 'claude', size: 'large', candidates });
  expect(large.name).toBe('big');
  expect(describeTaskChoice(large, 'large')).toContain('big');
  expect(describeTaskChoice(large, 'large')).toContain('week 60% · 5h 88% left');

  const small = await resolveTaskProfile({ app: 'claude', size: 'small', candidates });
  expect(small.name).toBe('little');
  expect(small.spent).toBe(false);

  // Codex reports no 5-hour window on some plans; the wording follows the
  // meters rather than promising a figure that was never read.
  expect(roomText({ h5: null, wk: 6 })).toBe('week 6% left');

  const blind = await resolveTaskProfile({ app: 'codex', size: 'large', candidates: [login('x', null, null)] });
  expect(blind.name).toBe(null);
  expect(blind.reason).toContain('Codex');

  const none = await resolveTaskProfile({ app: 'codex', size: 'small', candidates: [] });
  expect(none.name).toBe(null);
  expect(none.reason).toContain('No signed-in Codex login');
});

test("Codex's local login keeps its empty name through a resolution", async () => {
  const chosen = await resolveTaskProfile({
    app: 'codex',
    size: 'large',
    candidates: [{ name: '', label: "this machine's Codex login", room: { h5: null, wk: 95 } }]
  });

  expect(chosen.name).toBe('');
  expect(describeTaskChoice(chosen, 'large')).toContain("this machine's Codex login");
});

test('the flags name a login without naming a provider, and codex keeps its CLI', () => {
  expect(parseArgs(['--large-task'])).toEqual({ _: [], taskSize: 'large', provider: 'account' });
  expect(parseArgs(['--small-task'])).toEqual({ _: [], taskSize: 'small', provider: 'account' });

  // `bro codex --large-task` is the Codex logins, run through the codex CLI
  // like every other `bro codex …` route.
  expect(parseArgs(['codex', '--large-task'])).toEqual({
    _: [], provider: 'codex', harness: 'codex', taskSize: 'large'
  });
  // A later harness flag still wins.
  expect(parseArgs(['codex', '--small-task', '--omp']).harness).toBe('omp');
  // An explicit provider is never overridden by the flags.
  expect(parseArgs(['-p', 'codex', '--large-task']).provider).toBe('codex');
  // A login named outright stays named; the flag only chooses when none is.
  expect(parseArgs(['--account', 'work', '--large-task'])).toEqual({
    _: [], account: 'work', provider: 'account', taskSize: 'large'
  });
  // Everything after the flags still reaches the harness verbatim.
  expect(parseArgs(['--large-task', '--jev', '--print', 'hi'])).toEqual({
    _: [], taskSize: 'large', provider: 'account', jev: true, print: 'hi'
  });
});
