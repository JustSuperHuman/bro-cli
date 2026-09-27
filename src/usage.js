// Subscription meters across logins — what the Claude and Codex profile panes
// show beside each login, and the "what's left" summary above them.
//
// Every login's meters cost a network round trip (Claude's usage endpoint, or
// a short-lived `codex app-server`), so no menu waits for them: rows paint at
// once with placeholders and fill in as each login answers. Answers are kept
// for a minute, so the menus that follow a pick (resume destination, manage
// profiles) open with the numbers already in place instead of fetching again.

import { ICONS, ICON_WIDTH } from './icons.js';

const USAGE_TTL_MS = 60 * 1000;
const RESET = '\x1b[0m';
const DIM = '\x1b[2m';
const NORMAL = '\x1b[22m';
const RED = '\x1b[31m';
const AMBER = '\x1b[33m';
const GREEN = '\x1b[32m';

// --- one fetch per login, shared by every menu ------------------------------

const entries = new Map();

// Start (or reuse) one login's fetch. Resolves the meters, or null when they
// couldn't be read — never rejects, so callers can fan out without guards.
export function requestUsage(key, fetcher, { maxAge = USAGE_TTL_MS } = {}) {
  const hit = entries.get(key);
  if (hit && (hit.pending || Date.now() - hit.at < maxAge)) return hit.promise;
  const entry = { at: Date.now(), pending: true, value: undefined, promise: null };
  entry.promise = Promise.resolve()
    .then(fetcher)
    .then((value) => value ?? null, () => null)
    .then((value) => {
      Object.assign(entry, { at: Date.now(), pending: false, value });
      return value;
    });
  entries.set(key, entry);
  return entry.promise;
}

// What is known right now: undefined while the first fetch is in flight (or
// none was started), null when it failed, otherwise the meters.
export function peekUsage(key) {
  const entry = entries.get(key);
  return entry && !entry.pending ? entry.value : undefined;
}

export function clearUsageCache() {
  entries.clear();
}

// A select() `live` hook for menus whose labels read peekUsage(): repaint as
// each fetch lands, and stop once the menu has closed.
export function repaintOnUsage(promises) {
  return (repaint) => {
    let open = true;
    for (const promise of promises) promise.then(() => { if (open) repaint(); });
    return () => { open = false; };
  };
}

// --- per-login meters ---------------------------------------------------------

// A used percentage, coloured by pressure (green → amber → red) so the stats
// read at a glance instead of being one dim blur.
export function usagePercent(value) {
  if (typeof value !== 'number') return `${DIM}—${RESET}`;
  const pct = Math.round(value);
  const color = pct >= 80 ? RED : pct >= 50 ? AMBER : GREEN;
  return `${color}${pct}%${RESET}`;
}

// "5h 17% · wk 28% · Fable 53%" for `meters` given as [[label, used], …].
// Pending meters show a placeholder of the same shape, so a row doesn't
// change its layout when the numbers land.
export function metersText(meters, { pending = false } = {}) {
  return meters
    .map(([label, used]) => `${DIM}${label}${RESET} ${pending ? `${DIM}…${RESET}` : usagePercent(used)}`)
    .join(` ${DIM}·${RESET} `);
}

// Codex reports up to two rolling windows, and which one is which depends on
// the plan (Pro currently carries only the weekly one), so they are placed by
// length rather than by position. A window without a length keeps the
// historical order: primary is the short one.
export function codexMeters(summary) {
  const meters = { session: null, weekly: null };
  for (const [window, fallback] of [[summary?.primary, 'session'], [summary?.secondary, 'weekly']]) {
    if (typeof window?.usedPercent !== 'number') continue;
    const minutes = window.windowDurationMins;
    const slot = typeof minutes === 'number' ? (minutes >= 24 * 60 ? 'weekly' : 'session') : fallback;
    if (meters[slot] == null) meters[slot] = window.usedPercent;
  }
  return meters;
}

// --- what is left ---------------------------------------------------------------

const left = (used) => (typeof used === 'number' && Number.isFinite(used) ? Math.min(100, Math.max(0, 100 - used)) : null);
// An exhausted weekly allowance closes every shorter window along with it.
const gated = (value, weekly) => (value == null ? null : weekly === 0 ? 0 : value);
const tighter = (a, b) => (a == null ? b : b == null ? a : Math.min(a, b));

// Fable's allowance, on the plans that have one: "up to 50% of your weekly
// usage limits" (Anthropic, Claude Fable models on your plan). Its meter
// reads against that half, so one point of it is half a weekly point.
export const FABLE_SHARE = 0.5;

// What's left of the 5-hour window (`session`, in 5h points) when the week
// can only pay for `affordable` of them — known only once the account's
// 5h-to-week ratio has been measured (usage-history.js). `estimated` marks a
// figure the week cut down by that measure.
function windowLeft(session, weekLeft, affordable) {
  if (session == null) return { value: null, estimated: false };
  if (weekLeft === 0) return { value: 0, estimated: false };
  if (affordable != null && affordable < session) return { value: affordable, estimated: true };
  return { value: session, estimated: false };
}

// One Claude account's headroom, as a percentage of its own allowance per
// window. Only accounts whose usage reports a Fable limit can use Fable at
// all; the rest have no Fable headroom (null), not their overall headroom.
//
// With `ratio` — weekly points per 5-hour point, measured for this account —
// each 5-hour figure is what the window has left *and the week can still pay
// for*: an account with 95% of its window open but 1 weekly point left can
// really spend only 1 / ratio of it. Fable's week is the tighter of its own
// allowance and what's left of the week overall, in Fable units; its 5-hour
// figure is bounded by that, converted back through the ratio. Without a
// ratio, only an exhausted week closes the window.
export function claudeHeadroom(stats, { ratio = null } = {}) {
  if (!stats) return null;
  const wk = left(stats.weekly);
  const session = left(stats.session);
  const affordable = (weeklyPoints) => (ratio > 0 && weeklyPoints != null ? weeklyPoints / ratio : null);
  const fableWk = left(stats.fable) == null
    ? null
    : tighter(left(stats.fable), wk == null ? null : Math.min(100, wk / FABLE_SHARE));
  const h5 = windowLeft(session, wk, affordable(wk));
  const fable5h = fableWk == null ? null : windowLeft(session, fableWk, affordable(fableWk * FABLE_SHARE));
  return {
    h5: h5.value,
    wk,
    fable5h: fable5h ? fable5h.value : null,
    fableWk,
    measured: ratio > 0,
    estimated: { h5: h5.estimated, fable5h: Boolean(fable5h?.estimated) }
  };
}

export function codexHeadroom(summary) {
  if (!summary) return null;
  const { session, weekly } = codexMeters(summary);
  const wk = left(weekly);
  return { h5: gated(left(session), wk), wk };
}

// The headroom of each distinct login that answered. Two logins signed in as
// the same user draw from one allowance, so the second is skipped.
function distinctRooms(logins, headroom) {
  const seen = new Set();
  const rooms = [];
  for (const login of logins) {
    if (!login.stats) continue;
    if (login.identity) {
      if (seen.has(login.identity)) continue;
      seen.add(login.identity);
    }
    rooms.push(headroom(login.stats, login));
  }
  return rooms;
}

function sumWindow(rooms, key) {
  let left = 0;
  let count = 0;
  for (const room of rooms) {
    if (room[key] == null) continue;
    left += room[key];
    count++;
  }
  return { left, count };
}

// What each app has left across all of its accounts: per window, one 100%
// shared equally by the app's accounts that have that window, so no figure
// can pass 100%. Fable is Claude's alone and a 100% of its own, shared by the
// accounts that have Fable at all.
//
// `claude` and `codex` are [{ stats, identity, ratio }] — stats undefined
// while loading, null when unavailable; ratio as in claudeHeadroom. Each app
// comes back with its windows in whole percent (null when none of its
// accounts has that window), whether any of its logins is still loading, how
// many couldn't be read, and how many signed-in logins it has at all. Claude
// also says which 5-hour figures the week cut down by a measured estimate,
// and how many of its Fable accounts have no measure yet.
export function appHeadroom({ claude = [], codex = [] }) {
  const summarize = (logins, headroom, windows) => {
    const rooms = distinctRooms(logins, headroom);
    const app = {
      logins: logins.length,
      loading: logins.some((login) => login.stats === undefined),
      unavailable: logins.filter((login) => login.stats === null).length,
      estimated: {
        h5: rooms.some((room) => room.estimated?.h5),
        fable5h: rooms.some((room) => room.estimated?.fable5h)
      },
      // An estimate made with another plan's measure rather than its own.
      borrowed: rooms.some((room) => (room.estimated?.h5 || room.estimated?.fable5h) && room.ratioSource === 'plans'),
      unmeasuredFable: rooms.filter((room) => room.fableWk != null && room.measured === false).length
    };
    for (const key of windows) {
      const { left, count } = sumWindow(rooms, key);
      app[key] = count ? Math.round(left / count) : null;
    }
    return app;
  };
  return {
    claude: summarize(
      claude,
      (stats, login) => ({ ...claudeHeadroom(stats, { ratio: login.ratio }), ratioSource: login.ratioSource ?? null }),
      ['h5', 'wk', 'fable5h', 'fableWk']
    ),
    codex: summarize(codex, codexHeadroom, ['h5', 'wk'])
  };
}

// A figure, right-aligned in four columns and coloured by how much is left.
// It sets normal intensity itself: a menu paints its header rows dim, and a
// line that starts with an icon has no reset before its first figure.
// `estimated` prefixes ≈: the week cut the figure down, by a measured ratio.
function leftFigure(value, loading, { estimated = false } = {}) {
  if (loading) return `${DIM}   …${RESET}`;
  if (value == null) return `${DIM}   —${RESET}`;
  const color = value <= 20 ? RED : value <= 50 ? AMBER : GREEN;
  const text = `${value}%`;
  // A cut that still rounds to 100% is no cut worth marking — and ≈100%
  // wouldn't fit the column.
  if (!estimated || text.length > 3) return `${NORMAL}${color}${text.padStart(4)}${RESET}`;
  return `${NORMAL}${' '.repeat(3 - text.length)}${DIM}≈${RESET}${color}${text}${RESET}`;
}

const heading = (text) => `${DIM}${text.padStart(4)}${RESET}`;

// The summary above a profile list: a dim heading line, then one line per app
// with at least one signed-in login — its icon, what's left of its 5h window
// and of its week, and on Claude's line the same two for Fable:
//
//             5h   week              5h   week
//     ✻      74%    55%    Fable    96%     2%
//     >_       —    16%
//
// (the icons shown as the text marks terminals without images get). Each line
// is a function of the width available, like any picker label.
export function leftLines(summary) {
  const { claude, codex } = summary;
  const gap = (n) => ' '.repeat(n);
  const apps = [
    claude.logins && { icon: ICONS.claude, app: claude, fable: true },
    codex.logins && { icon: ICONS.codex, app: codex, fable: false }
  ].filter(Boolean);
  if (!apps.length) return [];
  const withFable = apps.some((row) => row.fable);

  const header = `${gap(ICON_WIDTH + 3)}${heading('5h')}${gap(3)}${heading('week')}`
    + (withFable ? `${gap(4 + 5 + 3)}${heading('5h')}${gap(3)}${heading('week')}` : '');
  const lines = apps.map(({ icon, app, fable }) => {
    const estimated = app.estimated || {};
    let line = `${icon}${gap(3)}${leftFigure(app.h5, app.loading, { estimated: estimated.h5 })}${gap(3)}${leftFigure(app.wk, app.loading)}`;
    if (fable) {
      line += `${gap(4)}${DIM}Fable${RESET}${gap(3)}${leftFigure(app.fable5h, app.loading, { estimated: estimated.fable5h })}`
        + `${gap(3)}${leftFigure(app.fableWk, app.loading)}`;
    }
    if (app.unavailable) line += `${gap(fable ? 3 : 4)}${DIM}${app.unavailable} unavailable${RESET}`;
    return line;
  });
  return [header, ...lines].map((line) => () => line);
}

// What the figures need explaining, if anything: a ≈ means the week cut that
// 5-hour figure down by a measured ratio (possibly another plan's); Fable
// accounts without any measure yet show only what their window has left,
// which the week may not cover.
export function leftNotes(summary) {
  const { claude } = summary;
  if (!claude.logins || claude.loading) return [];
  const notes = [];
  if (claude.estimated?.h5 || claude.estimated?.fable5h) {
    notes.push(claude.borrowed ? '≈ capped by the week (other plans\' ratio)' : '≈ capped by the week');
  }
  if (claude.unmeasuredFable) notes.push('Fable 5h: measuring');
  return notes;
}
